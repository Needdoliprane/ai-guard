# Lot P1 : ne pas inventer un résultat terminal runtime

29 octobre 2026. Périmètre exclusif : `src/runtime.rs` et nouveau `tests/runtime_terminal.rs`. Aucun changement runner, audit, API, main, deploy, smoke ou confinement. Aucun moteur réel, endpoint, conteneur ou volume appelé/modifié ; le moteur externe de régression est un exécutable **Rust** compilé dans un répertoire temporaire, sélectionné uniquement dans le PATH d'un processus de test enfant. Aucun script métier shell/JS/Python et aucune mutation globale unsafe de l'environnement.

**État final : voir le complément Podman après revue en fin de rapport.** Les sections initiales conservent le premier candidat et son historique RED/GREEN ; la validation finale accepte également `stopped` uniquement pour Podman et refuse les codes négatifs.

## Besoin, défaut reproduit et critère

AGENTS.md et `docs/engineering/CODING_RULES.md:8` exigent qu'une information inconnue ne soit jamais remplacée par zéro ou une preuve fictive. Avant correction, `RuntimeState` appliquait un défaut Serde à `ExitCode` et `Running`. Une inspection terminale sans ExitCode était donc convertie en `exit_code:0`. Après fin du client attach, `Runtime::execute` ne vérifiait que StartedAt ; il pouvait aussi retourner `Ok(ExecutionResult)` avec `Status=running` / `Running=true`.

Critère accepté : aucun résultat terminal retourné sans code de sortie observé et état exited non-running, avec fin runtime valide et ordonnée. Les commandes rapides déjà exited au premier inspect conservent leur démarrage historique et résultat, y compris échec non nul ; elles ne doivent pas être rejetées au motif qu'elles ne sont plus running.

## Preuve RED avant modification productive

- Baseline exact conservé : `/tmp/ai-guard-runtime-terminal-baseline-20261029.rs`.
- Régression via `Runtime::execute` réel appelant un faux CLI Rust externe : `/tmp/ai-guard-runtime-terminal-red.log`.
- Commande : `cargo test --locked --test runtime_terminal -- --nocapture`.
- Résultat : **FAIL**, 1 test agrégateur ; **8 sous-cas négatifs acceptés à tort**, 3 témoins positifs PASS.
- Sous-cas négatifs : ExitCode absent ; Running=true ; Status exited incohérent avec Running=true ; Status running incohérent avec Running=false ; Running absent ; FinishedAt absent, invalide ou antérieur à StartedAt.
- Le premier défaut affiche explicitement `Ok(ExecutionResult ... exit_code:0)` malgré l'absence d'ExitCode dans le JSON. Le second affiche `Ok(... status:"running", running:true ... exit_code:0)`.
- Témoins déjà PASS avant correction : sortie rapide 0, sortie rapide 7, clés d'inspection alias lowercase. Cela évite de confondre démarrage historique prouvé et processus actuellement actif.

## Correction minimale

`src/runtime.rs:236,242` : `Running` et `ExitCode` sont requis au parsing ; absence/null/mauvais type retourne l'erreur Result existante de parsing au lieu de fabriquer false/0. Le champ public reste i64 pour les valeurs réellement présentes ; aucun consommateur runner n'a été modifié.

`src/runtime.rs:251` : validation terminale privée retournant Result : status exactement exited, running=false, StartedAt et FinishedAt RFC3339 valides, début après l'époque et fin >= début. Tout autre état reste une erreur, sans promotion en résultat réussi.

`src/runtime.rs:475` : validation appelée sur l'inspection finale avant construction d'ExecutionResult. `has_started` et la collecte du démarrage historique restent inchangés ; StartedAt d'un job déjà exited est toujours accepté pour la preuve de début. Le code non nul du worker reste une donnée de résultat (ex. 7), pas une erreur de validation transformée.

Diff productif exact contre baseline : `/tmp/ai-guard-runtime-terminal.diff`. Aucun changement de cycle de vie, nom/volume, paramètres de confinement, commande moteur, callbacks ni politique de cleanup.

## Preuves GREEN et commandes constatées

1. `cargo test --locked --test runtime_terminal -- --nocapture` : **PASS**, 1 test, 11 sous-cas (8 refus attendus + 3 sorties rapides valides). Log `/tmp/ai-guard-runtime-terminal-green.log`.
2. `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_terminal --test runtime_security --test runtime_races --test runtime_authorization` : **PASS**, **20 tests**, 0 échec, 0 ignoré : terminal 1, sécurité 12, races 3, autorisation 4. Log `/tmp/ai-guard-runtime-terminal-targeted.log`.
3. `cargo clippy --locked --test runtime_terminal --test runtime_security --test runtime_races --test runtime_authorization -- -D warnings` : **PASS**. Log `/tmp/ai-guard-runtime-terminal-clippy.log`.
4. `rustfmt --check --edition 2024 src/runtime.rs tests/runtime_terminal.rs` : **PASS**.

Chaque sous-cas externe vérifie une création et un cleanup de son conteneur simulé, y compris en rejet. Les assertions sont faites après le retour d'execute ; le callback de test conserve les observations sans paniquer. Les témoins positifs vérifient un seul démarrage historique et la conservation exacte du code 0/7. Ce harnais prouve le chemin d'intégration de validation, pas une isolation moteur.

Empreintes gelées :

- `src/runtime.rs` : `1d56285947e0efd578db723e63a50977435f540e46136d4fcf6caa3eca440329`.
- `tests/runtime_terminal.rs` : `49c6bc45db49ffa3578f129dc005d377b1e442110868c3c5abe9c91ee419f173`.

## Limites et suites obligatoires

- **NOT_RUN dans ce lot** : vrai Docker/Podman, smoke E2E, confinement réel, suite globale/build release. Le parent assurera tests globaux, rebuild hôte/images et revue indépendante après ce gel, pour éviter un candidat incohérent.
- La validation terminale accepte exited ; les autres états ne sont pas silencieusement assimilés à une fin. Les preuves de compatibilité par moteur restent nécessaires.
- **Écart P1 non traité ici** : le cleanup du runtime précède encore la persistance terminale du runner. En cas d'audit indisponible, l'observation/identité durable, la réconciliation et le quota retenu restent le chantier registre/usage décrit dans `/tmp/ai-guard-worker-runtime-design.md`. Ce petit lot ne prétend pas conserver le terminal durable avant rm.
- Les dates runtime servent ici à refuser une fin incohérente ; aucune mesure CPU/RAM, facture, quota ou ledger n'est ajoutée.
- La revue indépendante est **PENDING** au gel de ce rapport ; aucun gate de release n'est déclaré fermé par le nombre de tests.

## Complément Podman après revue indépendante — 29 octobre 2026

La revue a identifié deux défauts dans le premier candidat : rejet d'un terminal Podman natif `stopped` et acceptation d'un ExitCode négatif pourtant inconnu. Les deux constats ont été vérifiés dans les sources officielles **v5.7.1**, puis reproduits avant correction :

- Podman distingue le processus terminé (`stopped`) du nettoyage de ses ressources runtime (`exited`) et sérialise cet état natif dans inspect. [Définition des états](https://github.com/containers/podman/blob/v5.7.1/libpod/define/containerstate.go#L21-L28), [conversion en chaîne](https://github.com/containers/podman/blob/v5.7.1/libpod/define/containerstate.go#L55-L60), [construction inspect](https://github.com/containers/podman/blob/v5.7.1/libpod/container_inspect.go#L117-L129).
- `ExitCode=-1` est utilisé lorsque le code réel ne peut être récupéré ; ce n'est pas une observation fiable de sortie métier. [Exit file absent](https://github.com/containers/podman/blob/v5.7.1/libpod/container_internal.go#L163-L174), [conmon terminé sans code récupérable](https://github.com/containers/podman/blob/v5.7.1/libpod/container_internal.go#L1404-L1412).

### RED puis correction

Baseline du premier candidat conservée : `/tmp/ai-guard-runtime-terminal-before-podman-review.rs`. Le harnais a été étendu pour choisir explicitement `RuntimeKind::Docker` ou `RuntimeKind::Podman`, chacun utilisant son exécutable fixture Rust propre dans le PATH enfant. **36 sous-cas** : les 11 cas initiaux par adaptateur, plus stopped valide 0/7, sentinelle négative exited/stopped, stopped encore running, fin absente ou inversée.

RED `/tmp/ai-guard-runtime-terminal-podman-red.log` : **FAIL**, 4 sous-cas incorrects (rejet stopped Podman 0 et 7 ; acceptation exited/-1 Docker et Podman). Les sorties rapides déjà exited restent PASS. Aucun vrai moteur Podman ni Docker n'a été lancé pour cette reproduction.

Correction minimale : `validate_terminal` reçoit désormais RuntimeKind ; `exited` accepté pour les deux, `stopped` accepté **seulement Podman** ; toujours Running=false et dates valides/ordonnées ; tout ExitCode négatif retourne une erreur, sans conversion vers zéro ou autre code. Les clés Running/ExitCode restent requises. Aucun changement de cleanup, runner, autorisation ou identité runtime.

Diff du complément seul : `/tmp/ai-guard-runtime-terminal-podman.diff`. Diff productif final contre baseline initiale : `/tmp/ai-guard-runtime-terminal-final.diff`. Le diff initial et tous les logs RED restent conservés.

### GREEN final et candidat gelé

- `cargo test --locked --test runtime_terminal -- --nocapture` : **PASS**, 1 test / **36 sous-cas** Docker/Podman, log `/tmp/ai-guard-runtime-terminal-podman-green.log`.
- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_terminal --test runtime_security --test runtime_races --test runtime_authorization` : **PASS**, **20 tests**, 0 échec/ignoré ; log `/tmp/ai-guard-runtime-terminal-targeted-final.log`.
- Clippy ciblé avec `-D warnings` : **PASS**, `/tmp/ai-guard-runtime-terminal-clippy-final.log`.
- `rustfmt --check --edition 2024 src/runtime.rs tests/runtime_terminal.rs` : **PASS**.

SHA-256 finaux, remplaçant les empreintes du premier candidat :

- `src/runtime.rs` : `6511248273ca87f0d6745c6fe1755aed51b3a1e2a6b576b11f8994cee205c28f`.
- `tests/runtime_terminal.rs` : `e0ee12d0ad67e31dee6ca1ed0fbf5f88b36e39d81ba2abe33f01e35bd55e041b`.

Relecture indépendante du complément : **PENDING** à ce gel. Les builds hôte/images et vérifications globales sont au parent après gel. Compatibilité **logicielle et documentaire seulement** ; qualification d'un moteur Podman réel **NOT_RUN/BLOCKED** selon disponibilité. Le travail ultérieur sur preuve terminale durable avant suppression et reprise/registre/usage reste explicitement ouvert.
