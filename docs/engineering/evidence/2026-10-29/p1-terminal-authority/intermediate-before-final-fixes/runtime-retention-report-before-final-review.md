# Lot P1 : conserver le terminal jusqu'à sa finalisation durable

29 octobre 2026. Implémentation approuvée par root, périmètre exclusif de cet agent : **`src/runtime.rs` et `tests/runtime_retention.rs`**. Root possède l'intégration runner/CLI, ses tests HTTP, les projections et documents du dépôt. Aucun moteur réel, endpoint externe, conteneur ou volume utilisé par les commandes de cet agent. Les processus « docker » ci-dessous sont des fixtures Rust compilées en répertoire temporaire, sélectionnées seulement dans l'environnement de processus enfants de test.

**État final : voir le complément « référence create inutilisable » en fin de rapport.** Les sections initiales conservent l'historique du premier candidat ; sa possibilité de fallback par nom a été reproduite puis supprimée avant qualification.

## Besoin et critère

Le runtime supprimait son conteneur avant de rendre le résultat au runner ; le runner ne pouvait donc engager la preuve terminale avant rm. Une finalisation d'audit refusée perdait la possibilité normale de réinspecter. Un échec de rm remplaçait aussi le résultat connu par une erreur sans accès à ce résultat.

Critère : `execute_retained` retourne un résultat terminal détenu sans suppression automatique ; l'appelant peut l'engager puis appeler un cleanup explicite/emprunté. L'échec de finalisation ou le Drop laisse l'instance inspectable. Une erreur après create acquitté tente un arrêt ciblé puis conserve l'instance ; si l'arrêt n'est pas observable, il reste inconnu. Une création échouée ne donne aucun droit d'adoption/kill/rm. Les helpers gardent leur wrapper de cleanup actuel.

## Baseline et RED avant changement productif

- Snapshot runtime : `/tmp/ai-guard-runtime-retention-baseline-20261029.rs`.
- `cargo test --locked --test runtime_retention -- --nocapture` sur l'API legacy initiale : **FAIL**, 5 sous-cas en échec, 2 témoins PASS. Log `/tmp/ai-guard-runtime-retention-red.log`.
- Défaillances observées : rm avant accusé terminal ; finalisation refusée sans instance conservée ; résultat inaccessible après rm échoué ; callback de démarrage refusé suivi de suppression ; absence de diagnostic d'arrêt inconnu lorsque le moteur est indisponible.
- Témoins préservés : conflit create ne touche pas l'instance étrangère ; cleanup automatique du wrapper legacy.
- Le parent a également reproduit indépendamment le défaut par son parcours CLI/gateway avant l'intégration ; ses preuves sont sous sa responsabilité.

## Implémentation

`RetainedExecution` (`runtime.rs:308`) détient un ExecutionResult et un token d'instance privé. Accesseurs publics convenus : `result() -> &ExecutionResult` et `container_name() -> &str`. Aucun champ dangereux public, aucun Clone/Deserialize/Default du token et **aucun Drop destructif**.

`Runtime::execute_retained` (`runtime.rs:517`) et l'ancien `execute` partagent `execute_attempt` et une seule boucle `execute_created` : start/attach, capture bornée, callbacks, deadline et validation terminale ne sont pas dupliqués. `execute` reste le wrapper à cleanup automatique pour compatibilité helpers/tests ; le nouveau chemin garde la preuve.

`Runtime::cleanup(&RetainedExecution)` (`runtime.rs:487`) vérifie le kind du token et supprime explicitement sa référence créée. Le résultat reste empruntable après succès ou échec du cleanup. La durabilité de l'accusé est appliquée par le parcours runner du parent ; cette API ne prétend pas vérifier cryptographiquement un accusé inexistant.

Le token est attribué seulement après create réussi. La référence retournée par create est capturée, bornée et validée comme argument sûr ; start/inspect/kill/rm post-create ciblent cette référence. Son format est volontairement opaque et ne prétend pas valider un ID physique hex64 : cela garde les fixtures antérieures compatibles. Un vrai CLI est l'autorité de la référence retournée. Si sa sortie est inutilisable, aucun start n'est émis ; l'instance créée reste sous son nom connu pour diagnostic et arrêt éventuel. Un create échoué/ambigu n'est jamais adopté par nom.

En erreur après création, `stop_retained_instance` inspecte l'instance ciblée ; si elle peut tourner ou l'inspection échoue, il demande kill puis réinspecte. Le message précise soit l'observation non-running, soit **`stop unknown`**. Cela ne fabrique ni terminal métier, ni code zéro, ni durée. L'instance n'est pas supprimée par execute_retained, même après callback refusé. Le wrapper legacy nettoie seulement les instances dont le create a été acquitté.

Diff contre baseline : `/tmp/ai-guard-runtime-retention.diff`. Aucun changement de réseau, UID/caps/limites, montage de projet original, profil, autorisation, schéma ou stockage. La validation terminale Docker/Podman du lot précédent reste active.

## Régressions GREEN

Le harnais `tests/runtime_retention.rs` vérifie **8 sous-cas dans 1 test** :

1. Aucun rm avant le marqueur d'accusé terminal ; cleanup seulement après.
2. Échec simulé de finalisation : terminal toujours inspectable ; Drop ne supprime rien.
3. rm retournant une erreur : résultat observé 0 toujours accessible et instance conservée.
4. Conflit create : aucune inspection/adoption/kill/rm de l'instance étrangère.
5. Callback start refusé : arrêt ciblé observé et instance conservée.
6. Moteur indisponible après callback : arrêt explicitement inconnu, aucune suppression.
7. Wrapper legacy : cleanup automatique conservé.
8. Substitution de l'alias de nom avant cleanup : suppression de la référence créée, autre instance préservée ; mutations start/kill/rm dirigées vers la référence retournée, pas l'alias remplacé.

Un premier passage ciblé a trouvé une instabilité du fixture dans le sous-cas 6 (`/tmp/ai-guard-runtime-retention-targeted.log`) : son fichier d'état pouvait être lu pendant une écriture par troncature avant le callback attendu. Le fixture effectue désormais ses transitions par écriture privée puis rename atomique ; il vérifie que le callback visé a été atteint. Le runtime n'a pas été assoupli. Tous les logs historiques sont conservés.

Commandes finales :

- `cargo test --locked --test runtime_retention -- --nocapture` : **PASS**, 1 test / 8 sous-cas, `/tmp/ai-guard-runtime-retention-green-final.log`.
- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_retention --test runtime_terminal --test runtime_security --test runtime_races --test runtime_authorization --test runner_terminal_audit` : **PASS**, **22 tests**, 0 FAIL / 0 ignored : retention 1, terminal 1 (36 sous-cas), sécurité 12, races 3, autorisation 4, intégration CLI/gateway parent 1. Log `/tmp/ai-guard-runtime-retention-targeted-final.log`.
- Clippy ciblé sur ces mêmes targets, `-- -D warnings` : **PASS**, `/tmp/ai-guard-runtime-retention-clippy-final.log`.
- `rustfmt --check --edition 2024 src/runtime.rs tests/runtime_retention.rs` : **PASS**. Aucun formatage d'un fichier possédé par root.

SHA-256 gelés :

- `src/runtime.rs` : `0d3874154a12f9ab5dd7cc40b88f4d2c376d1b5dfdc8c8a18f59984a48507a33`.
- `tests/runtime_retention.rs` : `01403ee9c0bd5d74231d9ab85f98d690a7ba4da8c775160dbb8b47c02f0a841d`.

## Limites précises et suite

- **PENDING** : revue indépendante du lot intégré, vérifications globales/build par root. **NOT_RUN** ici : effet sur vrai Docker/Podman, audit E2E conteneurisé, pannes réelles du daemon. Le harnais Rust ne qualifie aucun moteur.
- Le token protège l'ownership d'une instance acquittée dans le processus courant ; il ne livre pas encore une identité d'autorité/endpoint immuable, une tentative persistée avant create ni une reprise après crash. Un create dont l'acquittement échoue reste ambigu et n'est pas récupéré automatiquement.
- Le cleanup explicite n'est pas encore une opération durable idempotente. Le kind est vérifié ; un changement de contexte/daemon du même kind reste le chantier suivant. Aucune SQL lease n'est présentée comme fencing.
- En cas d'erreur après création, la preuve disponible est conservée, mais le suivi durable de l'instance/son quota et un résultat partiel structuré à réconcilier restent le lot registre. Si l'opérateur/moteur supprime externement l'instance, cette API n'invente pas la preuve perdue.
- Le wrapper legacy conserve volontairement son comportement de contrôle/helpers ; le worker métier doit utiliser le nouveau chemin et la finalisation du parent. Les données d'audit, secret masking et erreurs exposées du runner sont de son périmètre.
- Contrat concret préparé pour le registre : `/tmp/ai-guard-runtime-contract-next.md` (operation stable, référence réelle, observations CPU/RAM/dates/restart/image/autorité, primitives séparées, garde mono-hôte par tentative et start consommé non rejouable). Aucun registre, usage ou fencing multi-hôtes factice implémenté dans ce petit lot.

## Complément de revue : référence create inutilisable — 29 octobre 2026

Le premier candidat attribuait son token dès le statut CLI create réussi, avant validation de sa sortie, avec target initialisé au nom demandé. Une sortie vide, malformée ou tronquée pouvait conduire le chemin d'erreur à inspecter/kill ce nom ; le wrapper legacy pouvait aussi le supprimer. Si le nom avait été réaffecté, ces actions touchaient un successeur sans identité prouvée. Ce défaut a été corrigé avant la qualification.

Reproduction avant correction : `/tmp/ai-guard-runtime-retention-invalid-reference-red.log`. Les six sous-cas **empty/malformed/truncated × retained/legacy** répondent create avec statut succès, rendent le nom à un successeur running, puis constatent les actions indues sur ce successeur. **RED : 6 FAIL**, les 8 sous-cas précédents PASS. Baseline du candidat relu : `/tmp/ai-guard-runtime-retention-before-invalid-reference.rs`.

Correction : la référence retournée est entièrement validée **avant toute création du token CreatedInstance**. Si elle est inexploitable, AttemptFailure n'a aucune instance possédée : ni execute_retained ni execute legacy n'adoptent, inspectent, lancent, arrêtent ou suppriment le nom. Le diagnostic donne le nom demandé et **ownership unknown / runtime state unknown**, sans prétendre que la création a échoué ou que l'éventuelle instance inconnue est arrêtée. Aucun nouveau schéma hex64 n'a été ajouté : la validation opaque sûre reste celle approuvée.

Ces cas vérifient aussi que l'instance créée mais non identifiée reste en état created dans le fixture et que son successeur reste running inchangé ; aucune action automatique ne tente de résoudre l'ambiguïté par le nom. La réconciliation sûre de cette création ambiguë reste le chantier ultérieur, pas une suppression opportuniste.

Vérifications finales :

- `cargo test --locked --test runtime_retention -- --nocapture` : **PASS**, 1 test / **14 sous-cas**, `/tmp/ai-guard-runtime-retention-invalid-reference-green.log`.
- Suites ciblées retention/terminal/security/races/authorization/runner_terminal_audit : **PASS**, **22 tests**, 0 FAIL/ignored, `/tmp/ai-guard-runtime-retention-review-targeted.log`.
- Clippy ciblé `-D warnings` : **PASS**, `/tmp/ai-guard-runtime-retention-review-clippy.log`.
- Rustfmt check des deux fichiers possédés : **PASS**.
- Diff du complément : `/tmp/ai-guard-runtime-retention-invalid-reference.diff`. Les baselines, premier diff et logs RED/GREEN antérieurs restent conservés.

SHA-256 finaux remplaçant ceux du premier candidat :

- `src/runtime.rs` : `acd84dfb42d316bbb6536983f7d220dec7bf3e312c4b6a8e61e252cf04fe05b4`.
- `tests/runtime_retention.rs` : `6179750c135367cb91a5a32dcbe3f5ddc1b45785402e5a4c067975ee4654bd50`.

Recheck indépendant du complément demandé à `/root/review_front`, **PENDING** à ce gel. Aucun moteur, conteneur ou volume réel utilisé. Les limites de contexte/autorité, crash/reprise, registre/mesure et qualification production restent ouvertes.
