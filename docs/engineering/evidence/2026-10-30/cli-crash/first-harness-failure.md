# CLI crash / worker survival baseline

## Classification

**BLOCKED / NOT_RUN for the requested post-deadline survival observation.** One real worker was launched, observed Running, then reconciled and stopped. The harness rejected its Docker inspection response before recording a valid Running snapshot, so it did not wait past the five-second execution deadline or inspect the worker after that deadline. No survival or crash-supervision conclusion is drawn. No second worker was launched because the run instructions require stopping after a setup failure and prohibit loops.

## Environment and setup

- CLI: `/Users/louis/Documents/ai-guard/target/release/agentsandbox`; SHA-256 `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9` verified by the Rust harness before spawn.
- Docker executable: `/opt/homebrew/bin/docker`; Docker server `29.9.2`; endpoint `unix:///Users/louis/.docker/run/docker.sock`.
- Host: macOS arm64.
- Image: `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3`, image ID `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`, platform `linux/arm64`.
- New private project/config and SQLite gateway were created. QA credentials were generated locally, kept in mode-600 config files, and never printed. Child environments were cleared. No `.env`, real credential, provider, model, or external API was used.
- Launched CLI argv (nonsecret): `run --project <private-project> --config-dir <private-config> --gateway http://127.0.0.1:49225 --runtime docker --runtime-program /opt/homebrew/bin/docker --runtime-endpoint unix:///Users/louis/.docker/run/docker.sock --allow-rootful-runtime --image agentsandbox-worker:qa-managed-providers-20261030-ccgpe3 --profile observe --network none --timeout 5 -- /usr/bin/sleep 20`.

## What happened

The private session checkpoint recorded worker `a69d7608-d6b3-4fe7-b6c3-f25942738b89`, session `b55b8db7-503e-427b-a81a-b8b7086b1eb4`, container `953fe40fe742ed6797479c1bbe0260a827fd009c5b175db84ce18a109c701ad3`, and state `running`. The harness’s Docker inspection branch had already seen `State.Running=true`, `State.Status=running`, and a matching immutable container ID. It then incorrectly required Docker’s `.Config.Image` string to equal the requested mutable image tag. The admitted launch instead records the matching immutable image ID above (`LaunchSpec.network=none`, command `/usr/bin/sleep 20`). This was a harness validation error, not a product failure.

On that setup error, the harness sent SIGKILL only to its owned CLI PID and proceeded directly to the documented `worker-stop` cleanup; it did not wait seven seconds or make the required post-deadline runtime inspection. The run harness exited 1 because the expected baseline outcome was not recorded. The SIGKILL wait exit signal was not preserved in the summary, so it is not claimed as independently confirmed evidence.

## Cleanup and durable state

`worker-stop` exited 0 for the single session. Final `worker-status`: `terminal`, `quota_held=false`; the targeted container was absent after committed cleanup, and its named volume remained present. The post-cleanup control query returned `integrity_valid=true`, `event_count=18`, head hash `d3ea46aede3315862f1601d58c15f7707579b8a677892b90f37db56d1dc70e79`, `held_workers=0`, one usage record, and all usage entries `billable=false`. Before cleanup, the read-only query observed valid audit, 15 events, one held worker, and no usage record. The gateway child was stopped by the harness; project, config, audit DB, session checkpoint, volume, and evidence were retained.

## Evidence and limits

Private retained evidence directory: `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-cli-crash-baseline-fbGnzD` (mode 700; generated secret files and summary mode 600). The summary, exact run argv, private CLI logs, and generated config remain there. Private Rust harness and offline compile/run logs are under `/tmp/ai-guard-cli-crash-harness.zGERfc/` (mode 700). The project skill directories `.agents/skills/` and `.claude/skills/` were absent; the requested architecture, coding, and test contracts were read and followed.

This run establishes only that a real worker reached Running before the harness setup fault and that the exact job could be durably stopped and its quota released. It does not establish worker survival after CLI SIGKILL, survival past timeout, or a global runtime-security verdict.
