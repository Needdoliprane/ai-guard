# Lot runtime du registre — 29 octobre 2026

Portée : primitives Rust locales Docker/Podman et régressions logicielles, dans l'architecture approuvée. Aucune qualification production ou moteur réel n'est revendiquée.

## Changements

Fichiers possédés et modifiés : `src/runtime.rs`, nouveau `src/runtime/managed.rs`, nouveau `tests/runtime_managed.rs`, nouvelle fixture externe Rust `tests/runtime_fixtures/managed.rs`. Aucun fichier des autres agents, document du dépôt, Cargo, secret ou infrastructure n'a été modifié. Le checkout n'avait pas de HEAD et ses fichiers étaient préexistants non suivis ; ils ont été préservés.

- `ContainerSpec::with_operation(..., operation: Uuid)` et `.operation_id()` permettent de reconstruire une tentative persistée sans nouvel UUID. `new` garde une nouvelle opération par helper/export.
- `.with_managed_names(name, volume)` n'accepte que des noms bornés alnum/underscore/hyphen. Le nouveau parcours managed exige ces noms provenant du contrôle et une image immuable `sha256:<ID>` ; aucune sélection de bind/chemin hôte n'est possible.
- `Runtime` conserve son caractère Copy, son API historique et son noyau start/attach/timeout unique. `ManagedRuntime` fournit une invocation concrète figée vers le binaire résolu et un endpoint Unix explicite, ou Podman natif `local`.
- `ManagedRuntime::connect_docker`, `reopen`, `binding`, `doctor`, `resolve_image`, `create_named_volume`, `lookup_created`, `execute_retained`, `observe`, `stop_owned`, `resume_terminal`, `cleanup`, `cleanup_terminal` sont disponibles. `ContainerHandle` sérialisable garde ID complet, noms serveur, opération, session et fingerprint d'autorité. Les getters sont `id`, `name`, `operation_id`, `session_id`, `authority_fingerprint`.
- `execute_retained` fait create → inspect complet sélectif → callback FnOnce `on_created(OwnedContainerSnapshot)` → unique `StartDirective::EmitOnce` → start/attach. `ObserveOnly` et toute erreur du callback ne lancent rien. L'intégration du callback à la transaction Created+StartClaim et au verrou OS appartient au root/core ; le runtime ne transforme pas une lease SQL en fencing.
- Un create échoué ou une réponse inutilisable ne donne aucun droit d'adopter, inspecter, tuer ou retirer une instance par le nom. Après un create acquitté avec ID complet, toutes les commandes ciblent cet ID. Une erreur après acquisition tente uniquement l'arrêt ciblé et conserve l'instance, en distinguant arrêt constaté et arrêt inconnu.
- `lookup_created` n'émet ni create, ni start, ni kill, ni rm. Il recoupe inventaire ciblé réussi, ID complet, nom, image immuable, commande, volume et restrictions. Une erreur d'inventaire n'est jamais `None`. Même `None` n'est ni une preuve terminale ni une autorisation de relancer.
- Les snapshots exposent ID, nom, image ID normalisée, restart count, timestamps processus, code observé optionnel, limites CPU exactes par ratio entier/RAM/PIDs et diagnostics typés. Absence, sentinelle, illimité et incohérence ne deviennent pas zéro. Le JSON inspect complet, Config.Env et State.Error ne sont pas conservés ni affichés.
- Le callback de démarrage managed reçoit une nouvelle inspection complète, et non une conversion des bools de compatibilité RuntimeState : OOMKilled absent reste inconnu.
- La whitelist privée vérifie user 1000:1000, argv/entrypoint/workdir, réseau, read-only, non privilégié, cap-drop, no-new-privileges, restart=no, auto-remove=false, unique volume de travail et tmpfs bornés. Son hash `configuration_fingerprint` exclut Env ; chaque inspect doit conserver le même hash que created avant émission d'une observation running ou terminale.
- Une mesure terminale exige ID/image/limites/configuration inchangés, restart=0, état terminal reconnu par moteur, Running=false, code non négatif et dates valides ordonnées. `ExecutionResult.duration_ms` reste la durée attach/contrôle et n'est jamais utilisé comme allocation runtime.
- Cleanup garde résultat et observations accessibles, recoupe le terminal courant, puis utilise `rm ID` sans force. Le caller doit d'abord acquitter durablement son terminal/audit. Drop ne détruit rien. `cleanup_terminal(handle, committed)` permet la reprise après ACK sans fabriquer un ExecutionResult ni des captures vides.
- La création de volume managed exige des labels de contrôle non vides, une réponse non ambiguë et une inspection Name/Driver=local/Scope=local/Options vides/labels attendus. Un volume préexistant appartenant à une autre identité ou réalisant un bind hôte est refusé et conservé sans adoption/rm.

## Autorité Podman

Aucun Host.ID natif fictif. `connect_podman` renvoie l'erreur typée `ManagedRuntimeError::BootstrapRequired` et ne fait aucune mutation. `bootstrap_podman` est une opération explicite distincte, à invoquer seulement par root après autorisation durable. Elle crée un volume de contrôle `aig-authority-...` avec UUID/nonce privés, puis recoupe son Name/Labels. Le binding privé conserve graphRoot/runRoot, euid et environnement de sélection. `Debug` masque le marqueur ; les erreurs ne diffusent ni nonce ni diagnostic brut du CLI. Un marqueur absent, remplacé ou illisible reste une erreur d'autorité, jamais une disparition de worker. Aucun mécanisme de garbage collection du marqueur n'est déclenché automatiquement.

Pour Podman natif, `--remote=false` et l'environnement capturé sont figés ; graphRoot/runRoot sont réinspectés. Les flags `--root/--runroot` ne sont pas réinjectés : la documentation officielle précise que `--root` change le traitement des options de stockage. Pour les deux moteurs, les sélections de contexte ambiantes Docker/Podman sont retirées au profit de la cible explicite. L'autorité est vérifiée avant les mutations. Ces bindings mono-hôte ne constituent ni une identité workload cloud ni un fencing multi-hôtes.

Sources primaires consultées :

- Docker info expose réellement `ID`, notamment avec `--format '{{json .}}'` : https://docs.docker.com/reference/cli/docker/system/info/
- Structure native Podman HostInfo/StoreInfo, sans moteur Host.ID : https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/define/info.go
- Structures native inspect Podman (état, RestartCount, ressources, Tmpfs, Config, mounts) : https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/define/container_inspect.go
- Sélection Podman, `--remote`, `--url`, environnement et effet de `--root` sur options de stockage : https://docs.podman.io/en/stable/markdown/podman.1.html
- Inspection native Name/Labels/Options des volumes : https://docs.podman.io/en/stable/markdown/podman-volume-inspect.1.html

## Vérifications constatées

PASS — `rustfmt --edition 2024 src/runtime.rs tests/runtime_managed.rs tests/runtime_fixtures/managed.rs` ; seuls les fichiers possédés ont été formatés.

PASS — `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_managed --test runtime_retention --test runtime_terminal --test runtime_races` sur la source courante : 11 tests, zéro échec, zéro ignoré. Les 6 tests managed contiennent des matrices de snapshots et 18 cas de processus CLI externes (ack refusé, claim consommé, create ambigu/conflict, succès, audit start refusé, timeout, substitution de nom, changement moteur, mount/privilege/argv/image/tmpfs erronés, collision/bind volume, configuration changée après create, OOM absent restant inconnu dans le callback), ainsi qu'un scénario bootstrap/reopen/perte de marqueur Podman. Les fixtures sont des binaires Rust compilés dans les tests et ne qualifient aucun moteur.

PASS — `cargo clippy --locked --test runtime_managed -- -D warnings` sur la source courante. Un `cargo clippy --locked --all-targets -- -D warnings` a aussi passé plus tôt, avant les éditions API du root et les derniers renforcements ; le root refait les checks complets après intégration.

FAIL constaté, hors ownership — tentative `CARGO_INCREMENTAL=0 cargo test --locked` : lib 37 PASS, main 7 PASS, puis `api_security::denied_command_is_audited_but_never_executed_by_api` attendait count=1 et recevait 2 à cause du nouvel événement genesis. Le root/core a déjà reçu cette divergence. Log : `/tmp/ai-guard-worker-runtime-suite.log`. Le wrapper shell a ensuite rencontré le nom zsh réservé `status` ; cela n'efface pas le résultat Cargo enregistré. Aucun PASS global n'est revendiqué.

TDD : les RED réels bad_mount, volume_collision, bad_tmpfs, config_changed et missing_oom ont été observés avant leurs correctifs ; les API names/doctor/lookup ont d'abord échoué comme fonctionnalités absentes. Les deux premiers essais RED de la normalisation étaient empêchés par des éditions core non compilables (module absent, puis FromSql u64) ; une séquence RED propre pour ce premier bloc n'est pas revendiquée.

NOT_RUN — moteurs Docker/Podman réels, matrice Linux/macOS/ARM64, transports SSH/VM Podman, cloud/Kubernetes/NAS, test release. Aucun moteur ou service n'a été démarré ni modifié par cet agent.

## Intégration et limites à garder visibles

1. Le root doit persister l'opération et le binding en checkpoint privé avant create, fournir noms/volume provenant de l'admission authentifiée et utiliser l'image ID résolue dans le spec. Le volume doit porter authority/worker/namespace opaques issus du contrôle, jamais le tenant choisi par une requête.
2. Le root lie `on_created` aux transactions durables Created+StartClaim, ne produit EmitOnce que sur la première réponse fraîche, et ne rappelle pas le parcours create sur replay. Le verrou OS par tentative et les plans persistés appartiennent au root ; les tests multiprocessus de cette garde sont hors ownership runtime.
3. En reprise, `observe`/`lookup_created`/`resume_terminal` n'émettent aucun start. Comparer le terminal retrouvé au snapshot created durable avec `validate_terminal(kind, &created)` avant finalisation. Une création sans démarrage, une consommation start sans observation et une autorité inconnue conservent l'incertitude et le quota selon le contrôle.
4. `resume_terminal` renvoie uniquement l'observation runtime ; il ne prétend pas reconstruire stdout/stderr après un crash. Les résultats normaux restent détenus en mémoire et les instances/logs moteur restent disponibles après erreur, ACK refusé ou cleanup échoué. Le root doit conserver ses checkpoints de résultats et rendre explicite une sortie non récupérée.
5. Managed accepte Unix explicite et Podman natif `local`. SSH Podman/macOS VM et autres transports ne sont pas qualifiés par ce lot. L'API legacy Podman n'a pas été remplacée.
6. Le UID1000 et la propriété du workspace reposent sur l'image autorisée (/workspace possédé1000 dans l'image existante) et le copy-up/chown moteur. Les effets réels cgroups, copy-up et userns Podman doivent être qualifiés sur chaque moteur/hôte. Une inspection de configuration ne prouve pas ces effets.
7. Le second regard indépendant, l'intégration runner/API/Store et les preuves réelles sont conduits par le root. Les gates production restent ouverts tant que leurs preuves applicables manquent.
