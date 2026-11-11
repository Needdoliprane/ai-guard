# Prepared build/Compose source lifecycle — 11 November 2026

The frozen public-source composition passed **14 exact software parents** and an offline all-targets compile check. Root independently verified the source-only public tree against all 2,483 tested paths before adding this evidence. The 25 public D postimages are integrated with this record. This is not a release or a global NV-06 closure.

## Source and scope

Claude D prepares a private, filtered copy of the selected distribution before handing build/Compose paths to the engine CLI. Its retained consumer holds the input while observing the CLI process and its output pipes. Unknown collection returns failure and retains the copy for recovery. This does not establish what a daemon or a process that no longer holds either pipe is still reading.

Codex composed the 25 D paths by hunks, preserving public operator wires and guide changes. Two review observations were addressed: absolute source/TMPDIR paths now use the existing escaped, bounded text renderer; EN/FR guides distinguish collected status+EOF from an unknown end. Operational paths, descriptors, parsed manifest source and digest calculations are not rewritten into display text. One assert layout was formatted without changing its condition.

| Source identity | Value |
|---|---|
| Public base | `1dc676b103249171d584385060e3de3fedeea4bf` |
| Incoming D head / original base | `15b83b7f6b2b6a48638475f575c1d28805b5b2f9` / `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57` |
| Corrected D overlay | 25 files, 945,745 payload bytes; [per-file hashes](SOURCE_MANIFEST.json) |
| Overlay archive SHA-256 | `505c110b6bc9ffa3716e97526ebdc6ce07ae04eeaf906059af0f525ca0c11219` |
| Complete input | 2,483 regular files; USTAR 27,164,160 bytes |
| Input archive SHA-256 | `55f21aca978dbf65dbbff0957ff25f3a443d68cef5d8ca2f841f32e6086c2368` |
| Input manifest SHA-256 | `1474e06b8a965f59eea52d6af0407392733e82c608d65fd273acb6304defdef0` |
| Actual live-source export | 2,483 regular files; USTAR 27,330,560 bytes |
| Live-source export SHA-256 | `46bcff7483c40953a9e2c715c077652dc70546ebf7a9f82242b62bdc2e81fd91` |
| Verified public source-only tree, before evidence docs | `db9d53c759aaf2e7c974e4727c9b9c5a1b7e4fb2` |

All 25 postimages were checked against the frozen overlay and public preimages. The other 2,458 public files were preserved, with no deletion, private Full3 or unfinished Claude E source included. QA modes are 0644, derived from Git 100644; readonly archive metadata was normalized without content changes. Input and export have different tar layouts: the required equality is source path set, modes and content, not archive bytes. This record identifies the tested composition, not a future whole checkout or an approved commit attestation.

## Results and oracles

All 14 raw stdout/stderr streams, RCs and classifications were read for this summary. Each exact parent executed once, with one pass, zero failures and zero ignored; zero-match invocations are not counted. [TESTS.json](TESTS.json) gives exact names, actual argv, source hashes, times and raw-evidence hashes.

| Target | Passed parents | Behavior exercised |
|---|---:|---|
| Library source snapshot | 6 | Real control-character root names and unchanged filesystem/JSON bindings; relative-name escaping and truncation; private capture/release; root replacement; TMPDIR refusal; copied-entry integrity |
| `source_lifecycle` | 6 | Real CLI and compiled Rust synthetic consumer: prepared build/Compose inputs, original-source mutation after preparation, unknown collection, marker-write failure and failed removal |
| `retained_cli_lifecycle` | 1 | A dropped observer does not release material while the consumer uses it |
| `runtime_program` | 1 | The second image build revalidates the original engine identity |
| All-targets check | RC 0 | One successful Cargo `build-finished` record and no compiler error |

The unchanged R1 parent first establishes a real capture, verification, actual temporary-root binding, equal copied bytes, exact parsed manifest source and unchanged original. Its marker precedes the display assertions. It then verifies escaped provenance, retained location, a real missing-path refusal and test-owned cleanup. The existing relative-name parent protects the shared 1,024-character truncation and literal-backslash distinction.

Historical R1 remains separate: its test RC 101 reached the provenance control-character assertion; classifier RC 92 expected a boolean where `shared_writable` is a counter. Original **FAIL_HARNESS / INCOMPLETE** records remain unchanged. Independent reading of those same logs established `REGRESSION_REPRODUCED_FROM_EXISTING_LOGS` for provenance; it was not renamed `EXPECTED_RED`, and no second RED ran. The full unchanged parent now passed, including assertions not reached after that initial panic.

## Executed commands

These command forms ran inside the admitted QA against frozen source. Cargo resolved four actual libtest artifacts; their parent argv are in TESTS.json. The normal CLI artifact was separately resolved and checked executable for the integration parents, not used as a fifth test target.

~~~sh
/usr/local/cargo/bin/cargo test --frozen --offline \
  --lib --test source_lifecycle --test retained_cli_lifecycle \
  --test runtime_program --no-run --message-format=json

# Once per resolved libtest target:
<resolved-libtest-executable> --list

# Once per exact selector in TESTS.json:
<resolved-libtest-executable> <exact-selector> --exact --nocapture --test-threads=1

/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
~~~

Angle-bracket names denote recorded artifacts/selectors, not literal shell commands. The environment pinned Rust 1.99.1, explicit rustc, offline Cargo, jobs 1, incremental 0 and dev/test debug 0. Administrative bounds were 1,200 s for build, 60 s per library parent, 120 s per process parent and 900 s for check; they are not product latency objectives. No source, test, budget or command was retried or altered during the campaign.

Build ran 17:53:28–17:57:49 UTC, the 14 parents 17:57:50–17:58:50 UTC. Check finished at 18:00:18 UTC; exact-container closure completed at 18:00:35 UTC. Build, listings, artifact resolution, parent classifications, check, post-run source verification, real export and closure returned 0. Experiment and final RCs are 0 (`GREEN_OBSERVED`).

## QA profile and retained evidence

Admitted Linux ARM64 image: `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`. Profile: UID/GID 1000:1000, no network, readonly root, all capabilities dropped, no-new-privileges, admitted default seccomp, private IPC/cgroup namespaces, 4 CPU, 9 GiB memory, no container swap and 128 PIDs. Scratch was a 4 GiB tmpfs with 131,072 inodes. Only frozen inputs and public Cargo registry were mounted readonly; no engine socket, original project, model or real credential. Synthetic processes and real filesystem operations ran inside QA; no actual engine build, Compose effect or worker session was exercised.

Final cgroup memory was 2,067,824,640 bytes current and 6,752,890,880 bytes peak; `oom`, `oom_kill` and `oom_group_kill` were 0. These are compiler/QA values, not worker usage or a capacity measurement. Stop, stopped-state inspection, removal without force and the exact-CID process check completed; other resources were preserved.

Private raw evidence is retained as `qa-claude-source-lifecycle-20261111-green-r2`. Raw logs, source archives, credentials and private notes are not committed with this record.

| Retained private item | SHA-256 |
|---|---|
| Luna campaign report | `f4b78f477ba9c13543e2192baa7e709036d70d6fc6546a06f9609a4bf40f7990` |
| All-targets Cargo JSON | `5b9c17b26881f38497f1d59dc8f9100e44a3d61eb09cc51ac4820a72038d821e` |
| Independent historical R1 reassessment | `8324215f70b56838563c7eda07ec61a6abe160edbfa20a5b250910a397d0b25a` |

## Limits

- Docker/Podman/Kubernetes business execution, real image/Compose effects, native macOS, a distinct-UID source-writer boundary, daemon/auxiliary consumption after EOF, power-loss recovery, physical storage limits, 100-worker capacity and provider/model use are not qualified here.
- Source preparation is not an approved-commit attestation or an atomic filesystem snapshot. Same-size changes hiding timestamps retain the source review limit. SIGKILL/power loss may leave a partial copy for manual recovery.
- Global NV-06 and its deployment writer boundary remain open in the [audit register](../../../AUDIT_FOLLOWUP.md#registre-des-audits). No production gate closes. Clippy and a complete repository matrix were not run here.
- The separate Kubernetes API compilation failure is outside this D composition and is neither corrected nor qualified by this PASS. Private Full3 and unfinished Claude E remain independent.

See the [English recovery guide](../../../../setup/local/README.md#prepared-source-retained), [French guide](../../../../setup/local/README.fr.md#prepared-source-retained) and [delivery register](../../../CLAUDE_DELIVERIES_20261110.md) for contracts and integration status.
