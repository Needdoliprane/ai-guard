# Revue indépendante du lot moteur/logs

Date : 30 octobre 2026. État : PASS de la revue ciblée statique et des régressions logicielles recoupées, aucun défaut bloquant produit identifié dans ce périmètre. Le scénario Docker réel et la lecture de l'historique data668 ont également des preuves ciblées recoupées ci-dessous. Ce document porte uniquement sur le lot `fixed_sandbox_v2` et ses frontières de lancement, audit, captures et compatibilité. Il ne constitue ni l'audit global Daybreak, ni une qualification de release.

## Méthode et périmètre

Lecture seule des sources et preuves ; aucun build, test, moteur, fournisseur, navigateur, base réelle, credential ou contenu de log moteur exécuté/consulté par le reviewer. Le seul accès externe a été la lecture de sources officielles Docker/Podman pour recouper le profil stdin. Le dépôt est intégralement non suivi par Git ; la référence de comparaison est l'archive `/tmp/ai-guard-sqlite-freeze.DezAml/source.tar`, et les fichiers relus sont identifiés par SHA-256 ci-dessous, pas par un prétendu commit HEAD.

Instructions lues : AGENTS/RTK, architecture, règles de code, gates, stratégie de test, contrats tenant/local-cloud/metering. Le skill `security-audit` a été utilisé en mode guidance pour la revue ciblée. Les skills projet `.agents/skills` et `.claude/skills` ne sont pas présents ; aucun chargement automatique n'est revendiqué.

Sources relues : `src/control.rs`, `src/runtime.rs`, `src/runtime/managed.rs`, `src/runner.rs`, les chemins concernés de `src/runner/managed.rs`, `src/redaction.rs`, `src/audit/control.rs`, les tests core/runtime/reprise et leurs fixtures natives, et `docs/api.md`. Les sources non concernées et les autres gates ne sont pas déclarés couverts.

La référence finale de cette revue est exclusivement `/tmp/ai-guard-engine-logs-final.mGhUZD/source` et son archive SHA-256 `e58e387c47432e7335e73679e87deb1226e68b27827b406addec9b685b72cd18`. Le checkout poursuit ensuite un chantier TTY distinct : aucune modification postérieure de ce chantier n'est couverte ni assimilée au lot NoLogs.

## Résultat de la revue statique

Aucun défaut bloquant identifié dans le delta produit relu. Les points suivants sont établis par les sources, avec les limites dynamiques précisées plus bas :

- `SecurityPresetV1` conserve la représentation `fixed_sandbox_v1` et ajoute une variante fermée `fixed_sandbox_v2`. Le delta de `src/control.rs` contient seulement cette variante et deux méthodes non sérialisées. Les structures, ordres des champs et domaines de hash historiques n'ont pas changé. Les nouvelles admissions du broker demandent v2 ; une admission v1 explicite du contrôle de confiance demeure recevable.
- Toute nouvelle création, worker ou helper, passe `--log-driver none` dans `ContainerSpec::create_args`. Le preset v1 échoue avant création et avant `start_owned_retained`. Les arguments restent séparés du programme ; il n'y a pas de nouveau shell hôte.
- Le contrôle managé lit les faits avant le callback qui peut consommer le claim (`execute_owned`) ; le chemin public de compatibilité inspecte aussi le profil avant attach/start. Le driver, ses options et les booléens de flux sont requis et validés. Un driver différent, un champ requis manquant, un type incorrect ou une option active refuse le lancement.
- Run et Export exigent TTY/stdin fermés. Import conserve seul OpenStdin ; StdinOnce est vrai avec Docker et faux avec Podman. Ce détail a été corrigé dans le lot avant sa qualification finale. La [source Docker CLI v29.2.1](https://github.com/docker/cli/blob/v29.2.1/cli/command/container/opts.go#L337) active AttachStdin avec l'option interactive et [fixe StdinOnce quand OpenStdin et AttachStdin sont vrais](https://github.com/docker/cli/blob/v29.2.1/cli/command/container/opts.go#L686). La [structure d'inspection Podman](https://github.com/containers/podman/blob/main/libpod/define/container_inspect.go#L36) documente StdinOnce comme non pris en charge. Cela justifie le parseur ; cela ne qualifie pas un moteur Podman réel.
- Le digest privé v1 conserve exactement `ConfigurationFacts`, sa normalisation et le domaine `ai-guard:runtime-config:v1\0`. Le digest v2 a un domaine distinct et inclut logging, TTY, OpenStdin et StdinOnce. L'absence de version privée signifie v1 et reste omise à la sérialisation ; une version de handle non reconnue est refusée. Le contrôle de version et de digest lie les observations suivantes au Created original, sans recalculer ce dernier en v2.
- Le broker reprend le preset durable pour son spec et ses DTO. Un nouveau premier start v1 est refusé avant le claim et la commande start. `load_session` recharge le worker authentifié avant ce garde : un claim déjà consommé côté registre n'est pas traité comme une nouvelle autorisation. Les vieux handles v1 restent observables, stoppables et finalisables. Le code d'audit/DDL et les règles d'identité tenant ne changent pas.
- Les faits d'inspection sont typés ; `Config.Env` n'est pas désérialisé ni diffusé dans l'observation, et une erreur de parsing ne contient pas l'inspect brut. Le code de capture/masquage de `src/runner.rs` et `src/redaction.rs` est inchangé par rapport au gel. Les captures demandées passent par `captured_text` puis le masquage de l'événement avant checkpoint/audit ; `RuntimeCommandFailure` ne formate pas son stderr brut. Aucun nouveau stockage de sortie brute n'a été trouvé.

Le profil Podman accepte uniquement les métadonnées auxiliaires vides (Path/Tag) et Size absent, vide ou `0B`, avec aucune option Config active. Les champs sont corroborés par la [structure officielle InspectLogConfig](https://github.com/containers/podman/blob/main/libpod/define/container_inspect.go#L190). Une autre forme observée doit rester refusée tant qu'elle n'est pas qualifiée.

## Preuves recoupées

Les commandes sont exécutées par Luna/root, pas par le reviewer. Les logs nommés ont été lus directement. Un PASS ci-dessous est borné au scénario et au snapshot du log.

| Preuve | Statut constaté |
|---|---|
| Ancienne source isolée : `/tmp/ai-guard-engine-logs-red-20261030.log` | RED attendu : golden v1 PASS, deux tests v2 FAIL à la variante inconnue |
| `final-lib-log-policy-post-fix` | PASS : 3 tests, zéro ignoré, relance sur le dernier profil stdin |
| `final-lib-first-start-policy` | PASS : 1 test, zéro ignoré |
| `final-worker-registry` | PASS : 35 tests, zéro ignoré ; golden/replay v1-v2 et refus de substitution inclus |
| `final-workers-api` | PASS : 6 tests, zéro ignoré ; admission v1/v2, mismatch et inconnus inclus |
| `final-runtime-security` | PASS : 12 tests, zéro ignoré |
| `final-runtime-retention` | PASS : 1 test, zéro ignoré |
| `final-runtime-terminal` | PASS : 1 test, zéro ignoré |
| `final-runner-recovery-post-fix` | PASS : 16 tests, zéro ignoré, relance sur le dernier profil stdin |
| `final-runner-terminal-audit` | PASS : 1 test, zéro ignoré |
| `final-runtime-managed` puis `final-runtime-managed-single` | FAIL initial conservé : 13/14, assertion erronée de terminal dans le nouveau cas legacy. PASS du scénario corrigé : 1 test, 13 filtrés, zéro ignoré. Les 13 autres PASS du passage initial ne sont pas présentés comme réexécutés. |

Les noms courts `final-*` désignent `/tmp/ai-guard-engine-logs-final-<nom>-20261030.log`.

Commandes confirmées par Luna et leurs logs : `cargo fmt --all -- --check`, `cargo test --locked --lib log_policy_tests`, `cargo test --locked --lib first_start_policy_tests`, les cibles `cargo test --locked --test worker_registry`, `workers_api`, `runtime_managed`, `runtime_security`, `runtime_terminal`, `runtime_retention`, `runner_recovery` et `runner_terminal_audit`, puis `cargo clippy --locked --all-targets -- -D warnings`. Le rerun corrigé exact est `cargo test --locked --test runtime_managed managed_launch_requires_created_ack_and_keeps_owned_evidence -- --exact --nocapture`. Aucune variable de secret n'a été ajoutée par ce harness ; le PATH Rust/outils était explicite, avec shell login désactivé. `final-fmt-final` est vide et la réussite est attestée par l'exécuteur ; `final-clippy-final` montre la fin de vérification sans diagnostic.

Le FAIL `legacy_observe` ne prouve pas une régression produit : il exigeait un terminal pour une création refusée avant start, que `stop_owned` conserve correctement en Created. Le correctif vérifie maintenant Created non terminal, refus v1 sans callback/CLI, puis simule explicitement un ancien état exited avant validation v1. Il relit le journal de fixture final pour vérifier l'absence de start/rm. Aucun code produit n'a été modifié pour rendre cette assertion verte. La dernière lecture des logs confirme les PASS `single` (19,66 s), `lib-log-policy-post-fix` (0,01 s) et `runner-recovery-post-fix` (25,11 s).

La commande erronée visant une cible Cargo inexistante `runtime_log_policy` est conservée dans `/tmp/ai-guard-engine-logs-runtime-log-policy-20261030.log` comme NOT_RUN ; la vraie suite est le module lib `log_policy_tests`. Les premiers logs verts antérieurs au dernier delta stdin ne remplacent pas les logs `final-*`. Le premier essai RED ayant rencontré la compilation concurrente n'est pas une preuve de régression produit.

## Preuve Docker réelle ciblée

Le rapport `/tmp/ai-guard-engine-logs-real.md`, le log initial FAIL et le log `/tmp/ai-guard-engine-logs-real-docker-retry-20261030.log` ont été relus avec les assertions de `tests/runtime_terminal_persistence.rs`. L'exécuteur utilise Docker CLI 29.2.1 / Engine 29.9.2 sur Docker Desktop ARM64 et l'image worker existante `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`. Aucun modèle/fournisseur n'est appelé. Le fichier d'injection est une canary factice créée par le test et n'a pas été lu par le reviewer.

Commande ciblée : `cargo test --locked --release --test runtime_terminal_persistence rejected_terminal_audit_keeps_the_real_terminal_instance_inspectable -- --ignored --exact --nocapture`, avec runtime Docker, programme et endpoint Unix explicites, image préexistante et exception rootful explicite. Ce test normalement ignoré est effectivement exécuté : PASS 1, zéro ignoré, 9,51 s.

Résultats observables du cas final :

- Worker conservé `7acd3db951f90935c0466e22a06c4b711c3ecbfa34beda26c2977cbe6ed5a0d6`, session `0c19293e-1c34-4358-8f62-6eaf7341249c`, terminal exited/exit 0. Inspection ciblée : LogConfig.Type none et options vides, Tty/OpenStdin/StdinOnce faux.
- `docker logs` via le programme approuvé et l'ID immuable produit zéro octet stdout et 79 octets stderr, avec statut non-success. Le contenu stderr n'est pas retenu ; les assertions vérifient l'absence de la canary dans les deux flux. Cela prouve le comportement de lecture du journal pour ce worker avec none ; ce n'est pas une inspection de tous les stockages internes du moteur.
- La sortie capturée du broker est exactement `[REDACTED]` dans le checkpoint ; la valeur factice est absente du checkpoint, de l'API événements et de l'erreur runner.
- Un middleware refuse exactement une observation terminale HTTP 503 avant Store. Le worker reste InDoubt avec quota tenu, aucune allocation, audit valide avant/après réouverture, conteneur retenu et projet original inchangé. Ce résultat étaye l'ordre audit/terminal ; ce n'est pas une panne disque réelle.

Le premier essai `/tmp/ai-guard-engine-logs-real-docker-20261030.log` reste FAIL : la commande de canary finissait avant la fin du callback de démarrage ; le refus HTTP intervenait avant `finish_execution`, sans output_event à tester. La relance unique a brièvement retardé cette commande dans le worker afin de vérifier la capture terminale. Ce délai ne prouve pas un rendez-vous pour toutes les durées. Aucune modification de production n'a contourné l'échec.

Après le PASS moteur, une correction purement lexicale du guard Mutex dans le test supprime sa durée de vie à travers `await`. Le hash final est `816f8294b0fae4dfc40cbdcbb6b04b202c3dd02778d25c8472c827d4da2fb0d9`. `postedit-fmt` et `postedit-clippy` sont PASS, ce dernier via `cargo clippy --locked --test runtime_terminal_persistence -- -D warnings`. Aucun moteur n'a été relancé après cette modification du test ; le code de production et les assertions restent identiques.

## Compatibilité de lecture data668 et candidat déployé

Les fichiers `/tmp/ai-guard-engine-logs-final.mGhUZD/qa-before-upgrade.json` et `qa-after-upgrade.json`, collectés via les services réels avec une identité viewer selon root, ont été lus sans accès au contenu de la base ou aux credentials. Les deux réponses donnent une intégrité valide, exactement 668 événements et le même head `971b8923455befad82af3c0d1bad24c8d3b6a4d033e613ad1c2082c0d17fa021`. Le relevé avant comporte 19 workers tous terminal et quota tenu 0 ; celui d'après compte 19 workers `fixed_sandbox_v1` et quota tenu 0. Ces données étayent la lecture de l'ancien historique sans nouvel événement au passage de version.

`qa-stop.log` et `volume-consumers.txt` montrent le gateway arrêté ; une archive à froid est recensée avec SHA-256 `0c905707ea0a6db6a875b5173206104380097c9a33a3ba9dfebfb1d0d33cfef6`. L'archive de données n'a pas été ouverte par le reviewer. `control-image-build.log` produit l'image `sha256:33313bfcc385be5056e722cafd891f943e5cdb2ddf3849035a7767d2f1dcf543` ; `qa-start.log` atteint Healthy pour gateway, dashboard et ingress. Cela corrobore la mise à jour de la pile locale ; cela ne prouve ni une recette multiclient complète ni le cloud.

La preuve data668 est une preuve de lecture d'anciens workers. Elle ne prouve pas un nouveau first-start v1 autorisé, lequel reste refusé par le nouveau broker. La compatibilité d'un ancien claim consommé repose séparément sur le parcours de reprise et les tests revus plus haut.

## Parcours QA final : nouveau v2 et ancien v1 en lecture

Le rapport Luna `/tmp/ai-guard-engine-logs-qa.md` a été recoupé après sa mise à jour usage, avec une extraction de champs autorisés des fichiers `ai-guard-engine-qa-id-run.json`, `ai-guard-engine-qa-id-worker-status.json` et `ai-guard-engine-old-codex-status.json`. Aucun nouveau test, moteur, fournisseur ou endpoint n'a été exécuté par le reviewer. Le binaire utilisé est celui du gel final, SHA-256 `a4c2aab40b2b9184dac72301611286eb439fe9cedffffd231c4700db949b7c7a`.

- PASS : `/usr/bin/id`, strict, réseau none, provider generic, capture activée. La session `41eb7bb4-8f9d-4d63-b654-6057aff35b34`, worker `9856309f-4e30-4759-a091-418d5836e2dc`, tentative `1de39df2-3eb6-463e-90c6-3fa61595ed6e` se termine avec exit 0, sans timeout, cleanup removed et quota non retenu. Le statut confirme `fixed_sandbox_v2`, terminal et cleanup confirmé. Luna vérifie `uid=1000` par prédicat sans recopier la sortie complète. L'image ID du statut réel est `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`.
- PASS : l'ancienne session Codex `7c08e4e7-38b6-4bde-a978-376627c3bc6b` reste lisible par le nouveau binaire. Les champs de son statut confirment `fixed_sandbox_v1`, terminal, quota non retenu et cleanup confirmé. Cette opération est un worker-status en lecture ; aucun resume, export, stop ou first-start n'est demandé sur cette session.
- PASS selon le constat QA : l'audit reste valide, de 668 événements/head `971b8923455befad82af3c0d1bad24c8d3b6a4d033e613ad1c2082c0d17fa021` avant le nouveau run à 690 événements/head `40b460a20ea8bae0783893894d0a014205f1317b251ea8787a0f1755e6b5620b` après. Cette progression suit le nouveau run v2 ; elle est distincte de la conservation stricte des 668 événements lors de la seule mise à jour. Le README de la fixture garde son SHA-256 `4270d00d413082b6710f3b23fbf6bb34fb7ee669d67a6575c46e77152bf1721d`.
- PASS selon le relevé borné Luna : `GET api/usage?limit=100` retourne 20 allocations ; le filtre sur ce worker et cette tentative donne exactement une correspondance, `method_version=1`, runtime docker, provenance observed et `billable=false`. La première demande `limit=1000` avait reçu HTTP 400 ; la borne 1–100 est confirmée dans `WorkerPageQuery::parse` du `src/api.rs` gelé. Une seule demande corrigée a suffi, sans pagination. Ce relevé établit l'allocation de ce run, pas du CPU réellement consommé ni une facturation.

Ces preuves ferment les vérifications locales ciblées annoncées pour le lot NoLogs. La lecture historique v1, le refus logiciel d'un premier start v1 et le lancement réel v2 restent trois assertions distinctes ; aucune preuve de nouveau lancement v1 n'est revendiquée.

## Identité des sources relues

| Fichier | SHA-256 |
|---|---|
| src/control.rs | 5bb282d760f2160c2e019e747ee365323322aca9f223f843e8d7800ed3574e97 |
| src/runtime.rs | 1877c2c0e6b7d33e386ad1583c61ccdb871e3881e98227268577948851e36968 |
| src/runtime/managed.rs | 40cc5ee8b7815e36a2fe49c27b714babc36361e035e9769f7f7edaf245c8d771 |
| src/runner.rs | 74f962dbca664536530a1c827e2375489a81c8c83e9d668f60578f4f6aa44716 |
| src/runner/managed.rs | 849d37c8e411c6166fd3386349edb14a59756673d7277e9fe4b0d556c0ea288b |
| src/audit/control.rs | d034aafd0a376a6e3de05472b3430e53986e8b926742b3d1b9297dd3d2fba722 |
| tests/worker_registry.rs | 29563141968fb86568a1b4eeacf9c09896dcdce7acfa8bc3c3abb09d4d9dc49e |
| tests/workers_api.rs | 133f04528553a5cc25da25b621c5b5348d7bc1ccd828574bde29fd32a1d81afc |
| tests/runtime_managed.rs | a8d33ceebe7bd24e111482a2e6fb37195bc3305abb7f380f6738a36474d8e425 |
| tests/runtime_fixtures/managed.rs | d035ff6c5a608be615ffe3a44a71f2b3a5c6bde5ac6c3ef08c3bbe2a2f890815 |
| tests/runtime_fixtures/runner_terminal.rs | 103aecaed0ef9b7b34e2bf3abc7413bec067eef16ba7e7b45a422cb1076c5105 |
| tests/runtime_terminal_persistence.rs, après correction lexicale du guard | 816f8294b0fae4dfc40cbdcbb6b04b202c3dd02778d25c8472c827d4da2fb0d9 |
| docs/api.md | d39d0db1e237ff0c93ea5f4bf661c2d6036eac8acea09087bb8da5274e60fbad |

Les comparaisons `diff` avec l'archive gelée confirment `src/audit/control.rs` et `src/runner.rs` inchangés. Aucun fichier source, staging, commit ou donnée existante n'a été modifié par le reviewer ; seul ce rapport extérieur au dépôt est écrit.

L'archive finale `/tmp/ai-guard-engine-logs-final.mGhUZD/source.tar` porte le SHA-256 `e58e387c47432e7335e73679e87deb1226e68b27827b406addec9b685b72cd18`, vérifié indépendamment. Les fichiers runtime relus et le test terminal final correspondent aux entrées de cette archive. Les hashes de production ci-dessus sont identiques à ceux de la première conclusion de revue.

## Limites et suite de qualification

- NOT_RUN par le reviewer : toute exécution cible, la matrice complète, Docker/Podman/Kubernetes réels, fournisseur, navigateur, tests de charge et release.
- PASS recoupé : lecture de l'ancienne data668 à la mise à jour, scénario moteur Docker/canary, parcours v2 `/usr/bin/id`, statut historique v1 et relevé d'allocation de ce run, exécutés par root/Luna. Le reviewer n'a effectué aucune exécution cible ni ouvert la base.
- La baseline `/tmp/ai-guard-engine-logs-baseline.md` constate sur une instance Docker le driver local avec rotation native et 5 octets lisibles, sans en afficher le contenu. Elle ne permet ni l'allégation de rétention illimitée sur cet hôte, ni une vulnérabilité de fuite confirmée. Le lot traite le contrat de non-rétention brute avant masquage broker.
- Les sources et fixtures prouvent l'intention et les refus logiciels ; la preuve Docker établit l'effet ciblé sur la configuration et la lecture du journal de cette instance. Une preuve Docker ne qualifie pas Podman, Linux, une autre architecture ou Kubernetes. L'admission v1 via un contrôle de confiance et les anciens brokers restent hors garantie v2, explicitement documentés.

La conclusion finale de ce lot doit conserver les gates de production ouvertes selon leurs preuves propres. Aucun label « production-ready » n'est justifié par cette revue.
