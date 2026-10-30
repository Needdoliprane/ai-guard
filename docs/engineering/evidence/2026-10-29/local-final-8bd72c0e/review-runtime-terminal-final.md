# Revue indépendante — validation terminale runtime

29 octobre 2026. Périmètre en lecture seule : `src/runtime.rs` et `tests/runtime_terminal.rs`, comparés à `/tmp/ai-guard-runtime-terminal-baseline-20261029.rs` et `/tmp/ai-guard-runtime-terminal.diff`. Instructions AGENTS/RTK et contrats engineering lus. Aucun moteur, conteneur, volume, secret réel ou fichier source modifié ; seuls probes et rapport temporaires créés. Skills projet absents, aucun chargement prétendu.

## Verdict final du lot logiciel

**PASS après correction et revalidation des deux P1 ci-dessous.** La suppression des valeurs par défaut de Running/ExitCode et les vérifications de cohérence corrigent les défauts initiaux. Le complément traite les deux cas officiels Podman manquants, sans élargir les états terminaux acceptés chez Docker. Aucun défaut bloquant restant identifié dans ce diff. Ce verdict ne qualifie ni un moteur réel ni le produit.

### P1 CLOSED — Un terminal Podman valide `stopped` était rejeté

Le premier candidat, `src/runtime.rs:251-254`, imposait `Status == "exited"` sans tenir compte du moteur. Podman v5.7.1 distingue officiellement `stopped` (processus terminé) de `exited` (terminé et nettoyé), et inspect expose directement `runtimeInfo.State.String()`. Le rejet de `stopped` avec Running=false, code connu et dates valides constituait une régression du chemin Podman ; son occurrence exacte après start --attach reste à qualifier sur moteur réel. Sources primaires : [états Podman](https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/define/containerstate.go), [construction inspect](https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/container_inspect.go).

Correction attendue : validation adaptée au RuntimeKind ; conserver exited pour Docker, accepter aussi stopped pour Podman sous les mêmes preuves de fin. Aucun état actif ne doit être promu terminal.

### P1 CLOSED — La sentinelle `ExitCode=-1` était traitée comme un code observé

La présence d'un entier au parsing ne suffit pas à prouver un code métier. Podman v5.7.1 affecte -1 lorsque son exit-file manque ou lorsque conmon disparaît sans code récupérable ; un nettoyage peut ensuite laisser l'état exited. Le premier correctif acceptait cette valeur avec des dates valides et retournait `Ok(ExecutionResult { state.exit_code: -1, ... })`. Il ne fabriquait plus zéro, mais présentait toujours une observation manquante comme un code terminal connu. Source primaire : [traitements Podman de l'absence de code](https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/container_internal.go), notamment lignes 163–174 et 1404–1412.

Correction attendue : refuser la sentinelle négative, sans la convertir et sans déduire un code de sortie du seul client attach. Conserver les codes réels non nuls comme résultats métier, pas comme défauts de validation.

## Reproduction indépendante ciblée

Le test auteur a été copié dans `/tmp/ai-guard-review-runtime-terminal-probes.rs`, limité à trois sous-cas : `podman_stopped`, `podman_unknown_exit`, `fast_success`. Le moteur sélectionné est RuntimeKind::Podman et l'exécutable externe temporaire est un CLI Rust nommé podman. Le PATH est modifié uniquement dans l'enfant de test. Aucune commande de moteur réel n'est exécutée.

Compilation rustc édition 2024 `--test` contre `target/debug/deps/libagentsandbox-881ad779dc7021e6.rlib` et les dépendances existantes tempfile/tokio/uuid ; exit 0. Exécution :

```text
PATH=/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/Users/louis/.cargo/bin /tmp/ai-guard-review-runtime-terminal-probes --nocapture
case podman_stopped: FAIL
case podman_unknown_exit: FAIL
case fast_success: PASS
test result: FAILED. 0 passed; 1 failed; 0 ignored
```

Exit 101. Premier échec : `runtime did not prove a terminal exited state`. Second : `unproven terminal result was accepted: Ok(ExecutionResult ... state.exit_code: -1 ... exit_code: Some(0))`. Ce sont deux reproductions fonctionnelles, pas des erreurs de compilation. Le témoin rapide prouve que le harnais fonctionne avec un terminal valide.

## Autres critères examinés

- PASS par lecture : Running et ExitCode requis, refus des champs manquants/null/mauvais type via Serde. Docker et Podman les sérialisent sans omitempty ; cette exigence est compatible avec leurs contrats. [Docker v28.5.2](https://raw.githubusercontent.com/moby/moby/v28.5.2/api/types/container/container.go), [Podman v5.7.1](https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/define/container_inspect.go).
- PASS par lecture : has_started historique inchangé ; callback possible pour un processus déjà terminé, avant rejet éventuel du terminal. Dates RFC3339 et comparaison des instants avec offsets ; fin antérieure refusée. Aucune durée CPU/RAM ou facture n'est déduite.
- PASS dans les preuves auteur lues : 8 cas négatifs et 3 positifs dans le harnais externe ; 20 tests ciblés sécurité/races/authorization/terminal, Clippy et rustfmt. Ils ne couvrent pas les deux cas Podman reproduits ici.
- PASS par lecture : le diff n'altère ni les arguments de confinement ni la création, les callbacks, le kill ou le cleanup. Les erreurs terminales suivent le cleanup existant et ne deviennent pas un succès. La non-persistance du terminal avant rm reste un écart explicite distinct du prochain registre/usage.
- NOT_RUN par cette revue : vrai Docker/Podman, capture réelle des formats selon version, horloges moteur, smoke E2E, global tests/build ou qualification production. Une compatibilité documentaire ne qualifie pas le moteur.

Revue courte indépendante des contrats moteurs déléguée, puis sources taguées relues directement : aucun recours à une source secondaire.

## Correction et revalidation finales

Le complément est dans `/tmp/ai-guard-runtime-terminal-podman.diff`, le diff final contre baseline dans `/tmp/ai-guard-runtime-terminal-final.diff`. `validate_terminal` reçoit maintenant RuntimeKind. Il conserve exited pour les deux moteurs, accepte stopped seulement pour Podman, puis impose toujours Running=false, ExitCode >= 0, StartedAt/FinishedAt RFC3339 valides et fin >= début. Il ne remplace aucune valeur absente ou sentinelle et ne déduit pas le code métier du statut du client attach. `has_started`, les callbacks, les arguments moteur et le cleanup restent inchangés.

La source finale et la matrice de tests ont été relues. Celle-ci couvre chaque ancien cas pour les deux RuntimeKind, plus stopped succès/échec, stopped contradictoire/fin absente/fin inversée et les codes négatifs. En particulier Docker/stopped est refusé ; Podman/stopped avec code 0 ou 7 est conservé ; exited/-1 est refusé pour les deux.

**Recheck indépendant des mêmes probes**, recompilés contre l'artefact actualisé, sans modification du scratch de reproduction :

```text
/tmp/ai-guard-review-runtime-terminal-probes-fixed --nocapture
case podman_stopped: PASS
case podman_unknown_exit: PASS
case fast_success: PASS
test result: ok. 1 passed; 0 failed; 0 ignored
```

Compilation exit 0, exécution exit 0 en 0,35 s. Le même harnais qui échouait exit 101 valide donc la correction des deux défauts et son témoin positif. Les opérations restent des appels de `Runtime::execute` au CLI Rust temporaire, sans moteur ni volume réel.

Preuves auteur finales lues, sans les attribuer au reviewer : `/tmp/ai-guard-runtime-terminal-podman-green.log` : 36 sous-cas PASS dans 1 test ; `/tmp/ai-guard-runtime-terminal-targeted-final.log` : 20 tests PASS, 0 échec/ignoré ; `/tmp/ai-guard-runtime-terminal-clippy-final.log` : Clippy ciblé terminé ; rustfmt PASS consigné dans le rapport auteur. Aucun compte de sous-cas n'est présenté comme autant de tests indépendants ni comme une preuve d'isolation.

Empreintes finales relues et conformes au gel auteur :

```text
6511248273ca87f0d6745c6fe1755aed51b3a1e2a6b576b11f8994cee205c28f  src/runtime.rs
e0ee12d0ad67e31dee6ca1ed0fbf5f88b36e39d81ba2abe33f01e35bd55e041b  tests/runtime_terminal.rs
```

- PASS : revue logicielle finale, deux P1 traités et probes indépendants revalidés ; aucun défaut bloquant restant identifié dans ce lot.
- FAIL historique : deux cas Podman du premier candidat, reproduits puis fermés ; logs initiaux conservés.
- NOT_RUN : qualification Docker/Podman réelle, horloges moteur, smoke E2E, confinement, tests globaux/build par le reviewer.
- BLOCKED : qualification production globale selon les gates ouverts. L'écart connu de persistance terminale avant cleanup reste ouvert pour le chantier registre/usage ; il n'est pas résolu ni masqué par ce correctif.
- NOT_APPLICABLE : infrastructure distante, secrets, publication et facturation pour cette revue.
