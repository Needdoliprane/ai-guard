# Docker CLI interactive flags probe — 2026-10-30

## Résultat

**FAIL pour le contrat TTY réattachable ciblé** : Docker accepte la création avec `Tty=true`, `OpenStdin=true`, `AttachStdout=true`, `AttachStderr=true` et `LogConfig.Type=none`, mais configure aussi `StdinOnce=true`. Le conteneur reste à l’état `created` (il n’a pas été démarré). Cette configuration ne satisfait donc pas le critère demandé `StdinOnce=false`.

## Environnement et création

Docker CLI 29.2.1, serveur 29.9.2. Image préexistante `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`, image ID `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`.

Commande exacte, via endpoint Unix explicite :

```sh
/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock create \
  --name ai-guard-interactive-probe-20261030 \
  --label ai-guard.qualification=interactive-docker-flags-20261030 \
  --label ai-guard.purpose=tty-reattach-diagnostic \
  --interactive --tty --attach stdout --attach stderr \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges:true \
  --read-only --network none --cpus 1 --memory 128m --pids-limit 64 \
  --log-driver none --restart no --pull never \
  agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984 /usr/bin/id
```

Aucun volume, socket, projet, credential ou variable d’environnement n’a été monté/poussé. Le conteneur `ai-guard-interactive-probe-20261030` a été laissé créé, sans démarrage ni suppression, comme demandé.

## Valeurs inspectées (liste blanche)

`State=created`, `Tty=true`, `OpenStdin=true`, `StdinOnce=true`, `AttachStdin=true`, `AttachStdout=true`, `AttachStderr=true`, `LogType=none`, `LogConfig={}`. L’aide `docker create --help` confirme les options `--interactive`, `--tty`, `--attach` et `--log-driver`; elle ne documente pas de drapeau CLI pour forcer `StdinOnce=false`.

Preuves locales privées : `/tmp/ai-guard-interactive-docker-create.out`, `/tmp/ai-guard-interactive-docker-inspect.out`, `/tmp/ai-guard-interactive-docker-version.txt`. Aucune clé ni sortie du worker n’est présente.
