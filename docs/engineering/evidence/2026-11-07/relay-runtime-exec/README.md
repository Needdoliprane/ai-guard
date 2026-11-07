# Relay runtime exec port — software validation

Base `d550c34`, frozen source `relay-runtime-exec-fix73`. The three Rust files
and Cargo entries in [source.sha256](source.sha256) match the tested input;
EN/FR guides were completed afterward. This is a private transport component.
Its production permit factory remains closed until durable admission, retained
broker ownership and the original deadline are connected. No engine or cloud
campaign is authorized by these software results.

| Selection | Observed result | Parent log SHA-256 |
|---|---|---|
| Input67, seven runtime parents | 6 PASS,1 FAIL; one ignored child; RC101 | `07f5a6e0b310e85f17ea74cf915c197cc8fc40123730a1a8b983da205e563667` |
| Input73, same selection with EOF regression strengthened | 7 PASS,0 FAIL; one ignored child; tests27.05s,total141.78s; RC0 | `bc1222115f0a9089c941d64e3619c9e084f9064e8f9df5358506c073a7c3e4e4` |
| Input73, all-target compilation | RC0,total97.23s; no additional tests executed | `77133fe8ffa5ccf437d5a525a89e4d0fd288f50ca64b7988733ec728850f942a` |

The original echo parent completed readiness, then timed out waiting for EOF.
Tokio's Unix ChildStdin shutdown did not close the owned descriptor. The fix
drops the descriptor through the transport's borrowed Option, preserving stdout
for drainage. Repeated shutdown is idempotent; subsequent write/flush return
BrokenPipe. Neither test nor product deadlines were increased. Independent
source review accepted this correction and the unchanged business boundaries.

Commands, issued by Luna with the pinned Rust1.99.1 Cargo binary, were
`cargo test --lib --locked --offline runtime::managed::relay::tests -- --test-threads=1 --nocapture`
and `cargo check --all-targets --locked --offline`. Parent output is retained as
`runtime73-tests-am.log` and `runtime73-check-ao.log`. Each input had its own
scratch copy and target; the source manifest for73 has SHA-256
`fabe30c64857057fe15df39883d2b3ce17181d397f53bad66d20d51f766754ae`.

QA AM and AO used the pinned image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
Linux ARM64, UID1000, capabilities removed, no-new-privileges and seccomp,
no network except private loopback, readonly root/input/toolchain and registry,
4CPU/8GiB memory/256PIDs, bounded4GiB scratch and65536inodes. Target code ran
only there. Native tools performed source reading, hashing, formatting and Git.
The later AN check and initial AO fetch stopped before compilation on missing
public cache entries; their failures are not product failures. AO then used a
private scratch copy of the public cache with one locked, checksum-verified
Linux archive, and remained offline. AM's later QA exit137/OOMfalse is separate
from the successful test command; sources and parent logs survived closure.

Tests exercise actual OS pipes and bounded Rust child processes, closed engine
arguments, exact identity/configuration guards, inherited-environment rejection,
opaque bounded stderr and kill/join. Snapshots, runtime program/profile inputs
and engine observations are synthetic. They do not exercise a successful full
`open_relay_channel` against Docker or Podman, nor prove secret absence in an
actual exec environment. The existing worker terminal/quota are never inferred
from CLI exit or local EOF.

Real Docker/Podman, retained admission/RoleLease wiring, service recovery,
Kubernetes, A1/A2/B1,100workers and strict Clippy for this component remain
NOT_RUN or open implementation work. No global gate is closed.
