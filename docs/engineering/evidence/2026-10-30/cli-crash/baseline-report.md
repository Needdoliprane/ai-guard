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

## Addendum: authorized corrected-harness retry

After review found the image-tag check in the first harness attempt, one corrected retry was authorized and run exactly once. The retry compared Docker `.Config.Image` with the admitted immutable image ID and also checked exact CID, managed name, admitted network, and command. This corrected the setup error; the first attempt's output remains preserved above and in its own private evidence directory.

**Baseline result: FAIL (survival defect reproduced).** The release CLI hash remained `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9`. A single worker for `/usr/bin/sleep 20` was inspected Running with network `none`, admitted image ID `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`, matching managed name and immutable CID. The harness sent SIGKILL to only its owned CLI PID 1 ms after that Running observation; the child exit signal was 9. After 7.0019 seconds from the Running observation (worker timeout configured as 5 seconds), Docker inspected the same CID/name/image/network as `Status=running`, `Running=true`. This is the observed baseline failure requested by the maintainer; it is not a global security verdict.

The exact worker was then reconciled with `worker-stop` (exit 0): final state `terminal`, held workers `0`, container absent after committed cleanup, named volume retained. Post-cleanup audit was valid with 18 events and head `ff0aa03bf3892ad7f24d05588f29156ece58ba54470307f0b10b2013d09577b0`; one usage record was present and `billable=false`. Before cleanup, audit was valid with 15 events, one held worker, and no usage record. Project, config, audit, checkpoint, volume, and evidence remain retained.

Corrected retry evidence: `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-cli-crash-baseline-uT4aQG` (private root mode 700; key files and summary mode 600). Harness version manifest: `/tmp/ai-guard-cli-crash-harness.zGERfc/retry-manifest.json`; Rust source SHA-256 `aefb8a6fc8833b4f5309aba4a4d931cf651849aa9a16c5fb2360382274725745`. One worker run only in this corrected attempt; no loop, model, provider, external API, or Docker pruning occurred.
