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
