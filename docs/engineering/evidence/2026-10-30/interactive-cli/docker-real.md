# Qualification du CLI interactif Docker — harnais corrigé

## Préparation et build

Sources de test : `tests/runtime_interactive.rs`, `tests/runtime_fixtures/interactive_probe.rs`, `tests/runtime_fixtures/interactive.Dockerfile`.

- **PASS** — Rustfmt sur le test et le helper.
- **PASS** — helper Rust compilé seul sous Rust 1.99.1.
- **PASS** — `CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive --no-run`; binaire de test release produit.
- **PASS** — l’image de base `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984` a été inspectée avant build : `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`.
- **PASS** — image de test distincte construite localement avec `--pull=false --network=none`, sans conteneur worker lancé : `agentsandbox-worker:interactive-qa-20261030`, ID final `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`. Le contexte temporaire conservé `/tmp/ai-guard-interactive-build.Y6S2Wn` ne contient que le Dockerfile et la sonde Rust.
- **NOT_RUN** — les deux tests Docker ignorés attendent la revue/autorisation de run du parent.

Tentatives de build échouées avant la création de tout worker : le compilateur `rust:1.99.1-bookworm` n’était pas local et le helper Docker Desktop manquait au PATH; un contexte de repo complet était ensuite bloqué par `.dockerignore`; enfin, le flag rustc fut corrigé de `--opt-level=2` vers `-C opt-level=2`. Le build final évite l’image externe, compile avec le Rust déjà copié dans la base vérifiée, et n’a utilisé aucun credential affiché ou transmis. Le mainteneur a autorisé ce build local sous ces contraintes; le run réel reste en attente.

## Corrections de la revue

1. Le test lit les flags termios courants **avant** tout nettoyage. `Drop` restaure ensuite seulement le PTY privé. La comparaison stricte conserve `ICANON`, `ECHO`, `ISIG`, `IEXTEN`, les flags d’entrée/sortie/contrôle et les caractères; sur macOS, seul `PENDIN` peut différer. La cause documentée dans `/tmp/ai-guard-interactive-implementation-report.md` est l’ajout Darwin de `PENDIN` à la réactivation de `ICANON`; aucune erreur raw/echo n’est masquée.
2. Un middleware de test inspecte une whitelist de champs Docker (`LogConfig`, `Tty`, `OpenStdin`, `StdinOnce`, `Running`, `ExitCode`) sur l’observation terminale, avant ACK du vrai gateway. Il transmet le corps et la réponse sans les modifier et ne retarde pas cleanup après l’ACK. Seuls les booléens et le code de sortie sont conservés en mémoire; aucune inspection post-cleanup n’est supposée.
3. La sonde écrit `SAFE_BEFORE`, les séquences OSC52/DCS/titre/impression, puis `SAFE_AFTER`, et attend une deuxième ligne `exit`. Le harnais exige `SAFE_AFTER`, vérifie l’absence des séquences dangereuses sur les octets capturés, puis autorise la sortie 7. Le buffer plafonne à 2 MiB et tout overflow fait échouer explicitement le test.
4. Après le resize 120×40, le helper interroge réellement `stty size` jusqu’à observer `40 120`, sous délai de 8 s. L’assertion dépend de la dimension observée, non du temps écoulé.

## Cas préparés

1. `run --interactive`, `provider=generic`, `profile=observe`, `network=none`, sans capture ni credential. Le CLI tourne sous un controlling TTY foreground alloué au test, taille initiale 80×24. La sonde vérifie `isatty(0/1/2)`, lit une première ligne clavier, observe le resize, valide le framing filtré puis sort 7 après autorisation du harnais. Le test exige un CLI en échec, une observation Docker pré-ACK (`Tty/OpenStdin/StdinOnce=true`, logs `none`, stopped, exit 7), un événement d’échec, une allocation observée unique `billable=false`, quota libéré, audit valide après réouverture et projet source inchangé.
2. La sonde `wait` reçoit Ctrl-X deux fois. Le test exige un seul worker/claim et une seule autorisation d’attachement, une observation Docker pré-ACK avec paramètres interactifs et worker arrêté, quota libéré, audit réouvrable et projet source inchangé.

Les octets PTY ne sont jamais imprimés comme transcript. Le PTY isolé ne touche pas au terminal physique. Les dossiers QA, clés QA privées générées et DB restent conservés. Aucun moteur n’a encore été démarré par ces tests.

## Commandes après revue du parent

L’image construite est déjà disponible. Pour rejouer le build sans contexte de dépôt complet :

```sh
/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock image inspect --format '{{.Id}}' agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984
# doit égaler sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6
/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock build --pull=false --network=none --file /tmp/ai-guard-interactive-build.Y6S2Wn/interactive.Dockerfile --tag agentsandbox-worker:interactive-qa-20261030 /tmp/ai-guard-interactive-build.Y6S2Wn
/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock image inspect --format '{{.Id}}' agentsandbox-worker:interactive-qa-20261030
```

Puis, après autorisation explicite des deux runs :

```sh
export PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock \
AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 \
CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive -- --ignored
```

Limites : aucun résultat réel Docker/PTy n’est encore qualifié. Podman, autres plateformes, fournisseurs/TUI, perte du broker et réattachement après claim consommé restent hors de ce harnais.

## Dernière passe Clippy

Après correction des guards de mutex (scopes lexicaux clos avant tout `await`) et des comparaisons booléennes :

- **PASS** — `rustfmt --edition 2024 --check tests/runtime_interactive.rs tests/runtime_fixtures/interactive_probe.rs`.
- **PASS** — `CARGO_INCREMENTAL=0 cargo clippy --locked --test runtime_interactive -- -D warnings`.
- **NOT_RUN** — aucun run Docker n’a été déclenché; le premier test attend encore l’accord final root.

SHA-256 après correction :

- `tests/runtime_interactive.rs`: `185c7849a6fadc057f51937b2316560208a8845dc50a063158b730fe013db0a1`
- `tests/runtime_fixtures/interactive_probe.rs`: `e50b8353fb65c7d120fa89e50975f48e43cfd31357719e2a69f222c2210ca821`
- `tests/runtime_fixtures/interactive.Dockerfile`: `15466f59654927a6df1130543f38a3e18f0aa67fc984ae420611edafffa918e8`

Image probe toujours inspectée à `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

## Premier run Docker réel — FAIL, aucun rejeu

Préconditions juste avant l’exécution : SHA-256 `src/runtime/terminal.rs`=`851a9612118db27bf75ab89f8b05bc9c3fb7fb12800867ceddc146ad62d0e980`, `src/runtime.rs`=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, `tests/runtime_interactive.rs`=`185c7849a6fadc057f51937b2316560208a8845dc50a063158b730fe013db0a1`; image ID vérifié `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

Commande exacte lancée :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_cli_forwards_tty_input_resize_and_worker_exit -- --ignored --exact --nocapture
```

Résultat : **FAIL**, 1 test, 100.09 s. L’assertion du harnais n’a jamais vu `PROBE_READY` pendant son délai clavier de 100 s. L’erreur finale affichée est `could not verify restored PTY termios`; le chemin actuel de `run_on_pty` peut faire primer cette erreur de vérification sur l’erreur initiale de l’action clavier. Cette tentative ne prouve donc pas pourquoi le worker n’a pas atteint son probe; phase observée : après admission/import/claim, avant démarrage effectif du processus worker.

Dossier QA conservé : `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-hSFBHM`.

Diagnostic scalaire sans transcript ni credential :

- Docker CID complet `77b775398445accef9b5a87738a65802eae13fdbce42ab7f9eaa46b82519c19e` : `created`, `Running=false`, exit code `0`, `Tty/OpenStdin/StdinOnce=true`, log driver `none`, options `{}`. Le worker n’a pas démarré; aucune instance n’a été arrêtée ni supprimée.
- SQLite physique : `PRAGMA integrity_check=ok`. Worker `in_doubt`, `quota_held=1`, `start_operation_id` présent. Événements scalaires : `command.requested` 1, `security.decision` 1, `workspace.import.created/started/completed` 1 chacun, `runtime.submitted` 1, `runtime.terminal.attach.authorized` 1, `control.worker.transition` 6. Aucune allocation/observation terminale retournée par le test. La vérification applicative de chaîne d’audit (`Store::integrity`) n’a pas été exécutée hors du gateway après la fin du test.

Aucune correction du produit, reprise, stop, suppression ou deuxième run effectué. Attente de la décision root sur correction diagnostique du harnais et autorisation éventuelle d’un rejeu.

## Diagnostic offline et correction du harnais (sans nouveau run)

Checkpoint privé relu par champs `jq` allowlistés uniquement (aucun dump des options/config/clefs) : décision `allow`, règle `observation_only`; `worker.state=in_doubt`, quota retenu, start claim présent, aucune `last_observation`. Le motif persisté exact est `owned instance <CID> retained for diagnosis; runtime observed it not running: Invalid argument (os error 22)`. Docker garde le CID `77b775398445accef9b5a87738a65802eae13fdbce42ab7f9eaa46b82519c19e` en état `created`, `Running=false`, exit 0; paramètres TTY conformes, logs désactivés. Le défaut observé est donc antérieur au process worker et indépendant de la vérification TTY du harnais.

L’audit a ensuite été rouvert hors gateway via un petit utilitaire Rust temporaire lié au `Store` local, sans lire les payloads. Résultat applicatif : `audit_valid=true`, `event_count=15`, `workers=1`, `usage_rows=0`, `held_workers=1`; état worker `InDoubt`, quota tenu, claim et container présents. Le contrôle SQLite précédent (`PRAGMA integrity_check`) avait également retourné `ok`.

Le harnais a été corrigé sans toucher au produit et sans appeler Docker : un driver de test Rust intégré au binaire est session leader et conserve le controlling PTY après la sortie du broker. Il place le broker dans son propre process group foreground dans `pre_exec`, avant exec. Le parent attend l’ExitStatus du broker via une socket Unix privée, arrête puis draine la queue PTY, vérifie termios avant l’ACK au driver, puis le libère. Les loops de probe échouent dès qu’un status de sortie apparaît; l’erreur d’action, le status broker et l’erreur errno de vérification termios sont combinés au lieu d’être masqués. Aucun transcript n’est affiché.

- **PASS** — `CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive --no-run` après ajout du driver.
- **PASS** — `rustfmt --edition 2024 --check` des sources de test.
- **PASS** — `CARGO_INCREMENTAL=0 cargo clippy --locked --test runtime_interactive -- -D warnings`.
- **NOT_RUN** — aucun second worker, aucun start/stop/rm, aucune reprise du claim et aucun rejeu du test. Un nouveau run exige un feu vert root et une nouvelle session fraîche.

État retenu après diagnostic : conteneur toujours `created false 0`; aucune donnée supprimée. SHA-256 actuel : `tests/runtime_interactive.rs`=`ad328aa39708d26cecb56e6d670767d4cbe0060c46d4bee52aea051cc9082d99`, helper=`e50b8353fb65c7d120fa89e50975f48e43cfd31357719e2a69f222c2210ca821`, Dockerfile=`15466f59654927a6df1130543f38a3e18f0aa67fc984ae420611edafffa918e8`.

## Correction finale pre-retry : véritables fd 0/1/2

La revue a relevé que le driver ouvrait `/dev/tty` puis dupliquait cet alias pour le broker. Corrigé sans moteur : le processus driver reçoit le slave PTY sur ses trois descripteurs standard, et le broker utilise `Stdio::inherit()` pour conserver exactement ces fd 0/1/2. Aucun `/dev/tty` n’est rouvert pour les canaux du broker. Le test interne `interactive_host_driver` porte maintenant `#[ignore]`; le sous-processus le sélectionne explicitement avec `--ignored --exact`.

- **PASS** — formatage ciblé.
- **PASS** — `CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive --no-run`.
- **PASS** — `CARGO_INCREMENTAL=0 cargo clippy --locked --test runtime_interactive -- -D warnings`.
- **NOT_RUN** — aucun accès Docker et aucun rejeu. L’ancienne session/conteneur `created` reste intacte.

SHA-256 actuel du test : `a11ebf6cf6d8094b6e60000175836a1e728f3d74500716e14a74aec996e6b1ec`; helper et Dockerfile inchangés (`e50b8353fb65c7d120fa89e50975f48e43cfd31357719e2a69f222c2210ca821`, `15466f59654927a6df1130543f38a3e18f0aa67fc984ae420611edafffa918e8`). Image dédiée non modifiée : `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

## Nouvelle session du premier cas — FAIL harness, étape produit positive

Les préconditions étaient exactement celles du root : terminal=`5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, runtime=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, test=`a11ebf6cf6d8094b6e60000175836a1e728f3d74500716e14a74aec996e6b1ec`, image=`sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

Commande exacte :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_cli_forwards_tty_input_resize_and_worker_exit -- --ignored --exact --nocapture
```

Le test n’a pas terminé après plus de 6 minutes. Sur autorisation du root, le seul PID du test `89040` a été re-vérifié (binaire `target/release/deps/runtime_interactive-42ebfff7c42912ea`, filtre exact du premier cas), puis envoyé SIGTERM; Cargo s’est terminé naturellement avec le résultat signal 15. Aucun autre PID ni processus runtime n’a été arrêté. Un sample d’une seconde du PID montrait `__wait4` sur le thread du test, mais les symboles release ne permettent pas d’identifier la fonction Rust exacte. Le driver et le broker n’étaient plus présents.

Le parcours produit a effectivement atteint les faits terminaux, ce qui est une preuve partielle mais le test reste **FAIL** faute de rapport final/assertions complètes :

- Checkpoint scalar : `result.exit_code=7`, worker `terminal`, dernière observation `exited`, ACK terminal vrai, cleanup normal confirmé (`cleanup_removed=true`).
- SQLite : `PRAGMA integrity_check=ok`; réouverture via `Store::integrity()` : `audit_valid=true`, `event_count=25`, une worker terminale, une allocation, quota `held_workers=0`.
- Événements scalaires : `command.started` 1 et `command.failed` 1, `runtime.terminal.attach.authorized` 1; worker source terminal exit 7. Dossier QA conservé `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-PLdgfh`; nouveau CID `86c6a01276fd638cceb74f8a7bea3bf2cb3542317b5c442975383b1e7033caf3` a suivi le cleanup normal ACK du produit. Ancien CID `77b775...` non touché.

Aucun deuxième test, stop/rm manuel, provider ou autre moteur n’a été lancé. Un nouveau run nécessite diagnostic/correctif du blocage harness et une nouvelle autorisation root.

## Vérification du harnais sans moteur — PASS local seulement

À la demande du root, ajout d’un smoke sur le même driver PTY et `run_on_pty`, avec le vrai binaire `agentsandbox --help`, sans gateway, projet, admission ni worker. La capture et le contrôle avancent maintenant dans une boucle unique `poll` non bloquante; aucune thread de capture/statut ni `join()` n’est utilisée. Les attentes de statut broker, drain et sortie driver ont toutes des échéances monotones bornées. Le driver garde le controlling PTY jusqu’à l’ACK parent, après vérification termios.

Commande exacte :

```sh
PATH=/Users/louis/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive local_pty_host_driver_help_smoke -- --exact --nocapture
```

Résultat **PASS** en 0,34 s (un test). Phases scalaires observées : `driver_started` PID `93980`; `broker_status` code 0 / aucun signal; drain terminé avec 1672 octets privés; termios vérifié PASS avant `ack_sent`; `driver_wait_complete exited=true`; smoke terminé PASS. Aucun contenu capturé/transcript n’a été affiché. Le test vérifie également le marqueur `Usage:` sans l’imprimer.

Vérifications ciblées : `cargo fmt -- tests/runtime_interactive.rs`, `cargo test --locked --release --test runtime_interactive --no-run` et `cargo clippy --locked --test runtime_interactive -- -D warnings` passent. Cette preuve qualifie uniquement le cycle local du harnais et la terminaison du CLI `--help`; elle ne transforme pas les deux sessions précédentes en PASS et ne qualifie aucun comportement Docker/runtime. Aucun appel Docker n’a eu lieu dans cette étape. Aucun nouvel essai Docker n’est lancé sans feu vert root.

SHA-256 actuel : `tests/runtime_interactive.rs`=`188377b622ebb70e0e6e0707916b99c015ed795ff9f98ce61a96ac1c1c2e1a61`, helper=`e50b8353fb65c7d120fa89e50975f48e43cfd31357719e2a69f222c2210ca821`, Dockerfile=`15466f59654927a6df1130543f38a3e18f0aa67fc984ae420611edafffa918e8`.

## Troisième session réelle — premier cas PASS

Après vérification préalable des empreintes produit (`terminal.rs`=`5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, `runtime.rs`=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`), du test (`188377b622ebb70e0e6e0707916b99c015ed795ff9f98ce61a96ac1c1c2e1a61`) et de l’image (`sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`), le root a autorisé une session fraîche du seul cas exit7.

Commande exacte :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_cli_forwards_tty_input_resize_and_worker_exit -- --ignored --exact --nocapture
```

Résultat **PASS**, 1 test, 12,89 s. Dossier privé conservé : `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-rG4fBR`. Identifiants scalaires lus des colonnes dédiées SQLite : session `5d28f56a-352a-484f-a0af-4a30d780e9b7`, worker `9dbe3447-96af-4dec-8de2-cda01cfaa858`, CID `b7cf436ce49ea8a9b9d1b31c6a1e3e13f1c183d12836d68203db25597fbe177f`. État terminal, quota retenu `0`; une allocation, 25 événements; `PRAGMA integrity_check=ok`. Phases PTY : driver PID `94540`; broker code 1 (échec CLI attendu pour le code worker 7); capture privée drainée (783 octets, aucun transcript imprimé); termios restauré PASS avant ACK; driver sorti normalement. Assertions du test : saisie clavier acquittée, isatty fd0/1/2, taille initiale 80×24 puis 120×40 observée par le worker, worker exit 7 conservé comme échec CLI, séquences terminal dangereuses filtrées avant autorisation de sortie du helper, une allocation observée non facturable, quota libéré après ACK, audit valide après réouverture et projet source inchangé. La sortie de test confirme toutes ces assertions.

Les deux sessions précédentes restent consignées comme **FAIL harnais**; cette réussite est une session indépendante et ne les requalifie pas. Aucun autre cas, provider, browser, ancien claim/CID/volume ou run Docker n’a été touché. Le cas stop Ctrl-X reste **NOT_RUN**, en attente de l’autorisation root.

## Second cas réel — double Ctrl-X PASS

Préchecks effectués avant lancement : terminal=`5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, runtime=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, test=`188377b622ebb70e0e6e0707916b99c015ed795ff9f98ce61a96ac1c1c2e1a61`, image=`sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

Commande exacte :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_double_ctrl_x_stops_only_the_owned_worker -- --ignored --exact --nocapture
```

Résultat **PASS**, 1 test, 13,90 s, le 2026-10-30 vers 10:38 CET (mtime de la base QA 10:38:29 CET). Dossier privé conservé : `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-hvjuJb`. Identifiants scalaires depuis les colonnes dédiées SQLite : session `f13b97b2-eab6-4ac7-9cba-4ca60bcdffbc`, worker `7638fc7c-5a36-4246-b178-c38ead1745ce`, CID `244730457fed800a3ab5fd5fad2215acaa3c386aa3cef60d8692fb0915245d24`. État terminal, quota retenu `0`; une allocation et 25 événements; `PRAGMA integrity_check=ok`. Le test a rouvert le Store et validé l’audit applicatif.

Phases PTY : driver PID `95560`; broker code 1 (interruption attendue); drain 815 octets capturés en privé; termios PASS avant ACK; driver sorti normalement. Le test confirme STOP_READY, double Ctrl-X, état terminal observé par Docker avant ACK, quota libéré, une seule autorisation d’attachement/démarrage (aucune seconde autorisation), audit valide et projet source inchangé. Aucun second worker actif n’a servi de témoin; cette qualification ne prouve donc pas l’isolation face à un worker concurrent.

Versions relevées : Docker client `29.2.1`, serveur `29.9.2`; Rust `1.99.1`; hôte `Darwin 27.2.0 arm64`. `/tmp/ai-guard-interactive-ctrlx-cargo.log` est un **résumé scalaire rédigé après le run**, pas le stdout/stderr Cargo brut; il contient la commande, phases/résultat uniquement, aucun transcript PTY. La sortie Cargo originale du Ctrl-X n’a pas été enregistrée dans un fichier; elle demeure visible dans l’historique de l’appel outil. La sortie Cargo originale du premier PASS n’a pas non plus de fichier d’archive; les éléments de sortie consignés ci-dessus proviennent de l’historique outil et les identifiants ont été complétés depuis SQLite. Aucun retry ni troisième cas. Les deux FAIL du harnais antérieurs restent séparés et inchangés.

## Sonde raw-keys préparée — NOT_RUN

Ajout d’un mode helper Rust `raw-keys` : il vérifie ses trois TTY, sauvegarde le mode distant avec `/usr/bin/stty -g`, active `raw -echo` par arguments séparés, émet `RAW_KEYS_READY`, lit exactement huit octets, restaure le mode distant, puis écrit uniquement leur représentation hexadécimale fixe et une sentinelle. La séquence publique du test est `0d 0a 03 c3 a9 1b 5b 41` (CR, LF, Ctrl-C, UTF-8 `é`, flèche haut). Le test ignoré envoie cette séquence depuis le PTY local réel et exige l’écho hexadécimal exact avant de vérifier la fin propre du worker, les faits Docker pré-ACK, une allocation observée non facturable, le quota libéré, le projet original inchangé et l’audit après réouverture. Aucun parseur de touches ni fournisseur n’est ajouté.

Vérifications autorisées terminées : rustfmt du helper, `cargo test --locked --release --test runtime_interactive --no-run`, `cargo clippy --locked --test runtime_interactive -- -D warnings` PASS. Aucun lancement de test raw-keys/worker n’a été effectué; cas **NOT_RUN** jusqu’au feu vert root.

Base vérifiée avant build : `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984` = `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`. L’ancienne image de qualification exit7/Ctrl-X est restée inchangée : `agentsandbox-worker:interactive-qa-20261030` = `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`. Un premier build local sans pull/réseau a échoué uniquement car `Read` n’était pas importé pour `read_exact`; l’import a été corrigé, puis le build suivant a réussi :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin /opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock build --pull=false --network=none --file tests/runtime_fixtures/interactive.Dockerfile --tag agentsandbox-worker:interactive-raw-keys-qa-20261030 tests/runtime_fixtures
```

Nouvelle image probe uniquement, aucun conteneur démarré : `agentsandbox-worker:interactive-raw-keys-qa-20261030` = `sha256:c16828672d0e01049b91b8bbe3d993e213eff7557076a9552908dd24d1e795bb`. Le test et helper finaux ont les SHA-256 `tests/runtime_interactive.rs`=`78e18be1a808265dc95579b9d65549731d7d44e7aa742cc7a4222dbd6381d0f6`, `tests/runtime_fixtures/interactive_probe.rs`=`9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`; production `terminal.rs` et `runtime.rs` restent aux empreintes déjà gelées. Aucun Docker run, modèle, provider ou secret n’a été utilisé.

## Exécution autorisée raw-keys — PASS

Empreintes immédiatement avant run revalidées : production terminal=`5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, runtime=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`; test=`78e18be1a808265dc95579b9d65549731d7d44e7aa742cc7a4222dbd6381d0f6`; helper=`9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`; image=`sha256:c16828672d0e01049b91b8bbe3d993e213eff7557076a9552908dd24d1e795bb`.

Commande exacte exécutée une seule fois, sous timeout externe de 180 s :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-raw-keys-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_raw_keys_preserve_control_and_utf8_bytes -- --ignored --exact --nocapture
```

Résultat **PASS**, 1 test, 12,29 s, vers 2026-10-30 11:12 CET. Sortie stdout/stderr Cargo originale capturée en mode fichier privé (`0600`) dans `/tmp/ai-guard-interactive-rawkeys-cargo.log`; c’est le log brut du test, à l’exception que stdout/stderr sont fusionnés par `2>&1`. La sonde n’écrit aucun octet brut PTY dans ce log : uniquement marqueurs hexadécimaux et faits scalaires. Le résumé scalaire Ctrl-X précédent reste distinct et n’est pas un stdout brut.

Dossier QA conservé : `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-IulpqN`. Session `7ccca22c-3d66-4dc5-8469-b82cacf48177`, worker `c9715f2e-3a62-4e11-9e77-49ea64e95396`, CID `c7f31bbe2e471cd8ad682697a2081df2278b0251d297bbf60c10a0c8df0f4359`. PTY driver PID `8924`; broker code 0; drain privé 681 octets; termios hôte PASS avant ACK; driver sorti normalement. Le helper distant a lu et rapporté exactement `0d,0a,03,c3,a9,1b,5b,41` après configuration `raw -echo`, incluant Ctrl-C, et a fini avec code 0. Les assertions ont aussi validé terminal Docker avant ACK, une allocation observée `billable=false`, quota libéré, audit applicatif valide après réouverture et projet original inchangé. Contrôle SQLite scalaires : worker terminal, quota held `0`, une allocation, 25 événements, `PRAGMA integrity_check=ok`. Hôte/runtime relevés : Docker client `29.2.1`, serveur `29.9.2`; Rust `1.99.1`; Darwin `27.2.0 arm64`.

Aucun retry, second cas, provider, modèle ou ancien CID/image n’a été touché. Les deux FAIL initiaux du harnais et les PASS exit7/Ctrl-X restent dans leurs propres lignes de qualification.

## Renforcement Ctrl-X après revue — PASS, nouvelle session distincte

La revue a identifié que l’ancien test Ctrl-X n’excluait pas un arrêt dû au timeout de 120 s. Sans modifier la production ni les helpers, le test n’accepte désormais qu’un `command.failed` durable dont le résultat porte exactement `transport_stop_reason="operator requested worker stop with Ctrl-X Ctrl-X"` et `timed_out=false`, relu après réouverture SQLite. Il exige aussi état worker `Terminal`, une allocation unique observée non facturable, quota libéré, une seule autorisation attach/start, observation Docker terminal avant ACK, source intacte et statut du broker en moins de 30 s après la deuxième frappe. Cette borne exclut le timeout configuré; le test ne prétend pas garantir une latence sous 3 s.

Préchecks run : production terminal=`5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, runtime=`7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`; test renforcé=`3c8d7dfd48e1738f177493ccd2d49021ff2b3fed68f0f81775a34b4aee15810b`; helper inchangé=`9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`; image ancienne vérifiée=`sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`.

Commande exacte (une seule exécution; timeout externe 180 s) :

```sh
PATH=/opt/homebrew/bin:/Users/louis/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qa-20261030 CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive docker_interactive_double_ctrl_x_stops_only_the_owned_worker -- --ignored --exact --nocapture
```

Résultat **PASS**, 1 test, 13,57 s, vers 2026-10-30 11:15 CET. Sortie Cargo originale stdout/stderr fusionnée et archivée mode `0600` dans `/tmp/ai-guard-interactive-ctrlx-strict-cargo.log`. Dossier QA conservé `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-interactive-uMFdeZ`; session `6bed5970-514c-4863-8c23-d90169b6dec7`, worker `d3ea8183-b60a-4495-bbf1-13d3402cf79e`, CID `8f65474c8022d1a62d3a7df7adbea5edafdd4e393cc3618d3b2782c5e90b09c2`. Phases PTY : driver PID `11237`, broker code 1, drain 815 octets, termios PASS avant ACK, driver sorti. La base après run contient état terminal/quota held 0, une allocation, 25 événements, `PRAGMA integrity_check=ok`; extraction scalaires du seul événement `command.failed` donne le motif Ctrl-X exact et `timed_out=0`. Le test valide aussi l’intégrité applicative après réouverture. Le délai second Ctrl-X → réception du statut broker est mesuré dans le test et satisfait `<30 s`; la valeur numérique n’est pas imprimée, seul le temps total Cargo 13,57 s est conservé. Aucun autre cas/provider/worker n’a été lancé. Le PASS précédent à 13,90 s et les deux FAIL initiaux restent conservés comme preuves historiques distinctes; aucune affirmation de témoin concurrent n’est ajoutée.
