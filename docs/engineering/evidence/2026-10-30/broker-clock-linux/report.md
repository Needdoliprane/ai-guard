# Native broker clock — Linux ARM64 targeted qualification

Date: 2026-10-30. Scope: the already-frozen clock producer and `tests/broker_clock` only. This is a containerized Linux ARM64 test on Docker Desktop, not native Linux-host or broker-supervision qualification.

## Source and environment

- Frozen source archive: `/tmp/ai-guard-api-clock-qualified.wcFxCz/source.tar`, SHA-256 `3e1451dfb5ce997bb169b8fafef0529c5fd00266098c6c8c7677eff112bc1307` (matches the supplied archive identity). The tar has 115 entries: 100 regular files, no symlinks or other non-directory entries, top-levels only `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`, `tests/`; exact-component screening found no `.env`, `front`, `node_modules`, `target`, secrets/credential directories, SQLite files or private-key/certificate files. The two broad filename matches were ordinary source/test filenames (`src/provider/credentials.rs`, `tests/credential_handoff.rs`), not secret material.
- Selected source hashes in the extracted snapshot: `src/broker/clock.rs` `ec4b279749e502692dfd58b958efa53b2046ee58f651d5e26f7745e3190edd33`; `tests/broker_clock.rs` `96828d05b2ee6ee299a4b96f87c0cc644dd8234d708edeb935b58fd6a20be44a`; Cargo manifests match the frozen snapshot.
- Docker CLI `/opt/homebrew/bin/docker`, explicit socket `unix:///Users/louis/.docker/run/docker.sock`; client 29.2.1, server 29.9.2. Existing cache image `agentsandbox-sqlite-tests:qa-20261030-8QMTSx`, ID `sha256:07f9d4821dc1acc9c882378cee7e64ff0f69d94b73d2e8c05719913610317f24`, Linux ARM64, user 1000:1000.
- New isolated context `/tmp/ai-guard-broker-clock-linux.xOuU0P`; archive extracted there with no repository path or mount. Dockerfile, logs, selected hashes and inspection outputs are mode 600 inside the private directory.

## Commands and results

Build command:

```text
docker --host unix:///Users/louis/.docker/run/docker.sock build --progress plain --network none -f /tmp/ai-guard-broker-clock-linux.xOuU0P/Dockerfile -t agentsandbox-broker-clock-linux:qa-20261030-intent /tmp/ai-guard-broker-clock-linux.xOuU0P
```

Build PASS, exit 0. Dockerfile copies the frozen source with `--chown=1000:1000`, uses fresh `/build/source`, keeps `/build/target` only as the pre-existing Cargo cache, runs as UID/GID 1000, and executes `cargo test --locked --offline --no-run --test broker_clock`. No network was available or needed.

One new container was created and retained: `ai-guard-clock-linux-intent-20261030-xOuU0P`, ID `889779a4e2dc1462d9d01b295803f6ea7243e9d8c6d78e34c952c67100195f7d`. It used `--network none`, `--cap-drop ALL`, `--security-opt no-new-privileges`, `--cpus 2`, `--memory 2147483648`, `--pids-limit 256`, `--user 1000:1000`, no mounts and no published ports; `Privileged=false`. The selected before/after Docker inspections preserve these values. It exited naturally with code 0 and was not removed. The new image is retained as `agentsandbox-broker-clock-linux:qa-20261030-intent`, ID `sha256:8e48e6c9b949d58cef12237644a3b13e2ef8448666cf2d6b732473f2a96072aa`.

Container command:

```text
uname -a; rustc --version; cargo --version; cargo test --locked --offline --test broker_clock
```

Observed environment: Linux `7.0.14-linuxkit`, `aarch64`; Rust `1.99.1 (58cf8f9c3 2026-10-13)`; Cargo `1.99.1 (5f94df478 2026-08-27)`. Test result: **5 passed, 0 failed, 1 ignored**. The ignored entry is the private child probe function, invoked by the parent test to sample a second native Rust process; it is not counted as a separate product test pass.

Original build log: `/tmp/ai-guard-broker-clock-linux.xOuU0P/build.log`. Original run log: `container.log`. Bounded container inspection: `container-inspect-before.txt`, `container-inspect-after.txt`; version/image facts and file hashes are in the same evidence directory. All build, test, inspect and source manifests remain available there.

## Limits

This qualifies the producer and its focused test on this Linux ARM64 container only. Linux x86_64, native Linux host, Podman, sleep/wake, reboot, syscall-error injection, clock rollback/boot change handling by a consumer, deadline enforcement, watchdog behavior, D5 fencing, and actual worker/broker execution are **NOT_RUN**. No model/provider, worker, external API or E2E was invoked. No repository file, existing image, container, or volume was modified or deleted.

## Artifact recopy and private directory permissions

After the initial run, the extracted QA-only source directory was found at mode `drw-------` (600), which made the evidence directory unreadable as a directory. Its directories were corrected to 700; regular files remain 600. No archive or source contents were changed.

The four selected files were copied out of the stopped, retained container to a new private directory `/tmp/ai-guard-clock-linux-recopy.ieHti5` (directory and files mode 700/600). Their hashes match the host extraction manifest above exactly:

| File | Re-copied SHA-256 |
|---|---|
| `src/broker/clock.rs` | `ec4b279749e502692dfd58b958efa53b2046ee58f651d5e26f7745e3190edd33` |
| `tests/broker_clock.rs` | `96828d05b2ee6ee299a4b96f87c0cc644dd8234d708edeb935b58fd6a20be44a` |
| `Cargo.toml` | `55f7ee3de152374b42dd219ec98e004905804879b2677a641e3af434309ea13a` |
| `Cargo.lock` | `8f6a66b3ea75d5b26beb2365d78dec5e34479d19863fd29c22b4b37450f66d31` |

This recopy confirms the selected build/test inputs inside the retained container matched the host extraction. It does not expand the platform or runtime claims above. The original build/test logs and inspections were not rewritten.
