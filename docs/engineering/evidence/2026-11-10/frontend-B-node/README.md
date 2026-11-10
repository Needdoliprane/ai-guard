# Frontend B Node evidence — 10 November 2026

**Unmerged candidate only.** These results do not describe the public frontend
or qualify the complete B delivery. The candidate starts at public `2cc29ee`,
with the forty-path composition and a targeted timer fix; only its 61 frontend
source files were admitted to this Node QA. Historical B results are not transferred.

## Sources and observed results

| Freeze | Source-manifest SHA-256 | Result |
|---|---|---|
| Timer RED, 61 files | `9638141176ea6385f077a5b15c1c009fd1e4de25d974a78e451261b0f32a830e` | Exact closure-timer parent RC1, assertion `1 !== 0`, 21:15:19 UTC. |
| Timer FIX, 61 files | `6df576b11726ecde8e5e6f45752e570da6d5efa1cc76c80ae49751c4d4b55a62` | Same parent alone RC0, 21:16:55 UTC; later frontend batch/build below. |

The manifests differ only at `front/src/journal-store.ts`. The selector is
« la fermeture annule une relance du direct déjà armée et ignore son callback tardif ».
The RED failure and FIX result are retained separately.

The six-file Node batch, 21:20:55–21:20:58 UTC, returned RC0:
80 tests, 79 passed, 0 failed, 1 skipped, 0 cancelled. Its attempted negative
filter did not exclude the timer, which passed again. It is counted only once:
**79 unique passed titles**, not 80 unique passes after adding the solo result.
The skipped title is “the real newest-first pages decode, and the real
absent-route answer equals a real resource refusal”: the real backend was
not supplied, so that path remains NOT_RUN. No rerun was made to repair selection.

Effective versions were Node 24.22.2 and npm 11.19.1. The 21:23:54–21:23:57 UTC
build returned RC0 for `tsc --noEmit` and Vite, with 23 modules transformed.
It used a fresh scratch output directory and retained the warning that outDir
is outside the project root. This is a static-build result, not browser execution.

## npm capture gap retained

The original `npm ci` invocation was made once. The tool observation returned
RC0 and “added 14 packages in 715ms”, but the command did not capture start/end
UTC or separate raw stdout/stderr at process time. Those fields remain
NOT_CAPTURED; the later explanatory files are explicitly not raw captures.

A later actual npm-tree inspection returned RC0. Root separately verified
the installed fourteen packages, their lock bindings/SRI and all three locks.
These checks do not manufacture the missing original process captures. The
installation was not repeated to obtain a cleaner record.

## Post-source, exports and closure

Root verified each of the 61 source hashes, exact pathsets, zero nonregular
source entries and fourteen exact packages after execution. Actual regular-only
exports, preserved privately:

- RED source: `9a045882853bc041aaad634da0f93fc5182ec7d269785ffd67e119ecde34513b`, 1,198,080 bytes.
- FIX source: `1b3b5ef051b4ab26be9bac3eb4692ac9cf10b25e9ebe1d93c30edf1ef2c7dac6`, 1,198,080 bytes.
- Built dist: `02a96e5db59cd30bf876ec16115980c7c3f3ff8241fe590a8c5bc9dc0c3fb209`, four files, 296,960 bytes.

The owned QA was stopped and removed normally after exports. Only init/sleep
were present before the stop; its exit 143 and OOM false belong to that idle
closure, not the tests. The watch returned terminal 0. Closure record SHA-256:
`bb131c62130ef3b027e922eaffb1bff42b90dd09faf5d9ee00fa5fcf6d087480`.
Peak memory 442,789,888 bytes/OOM 0, scratch 62,074,880/2,147,483,648 bytes and 1,280/65,536
inodes are environment observations, not performance or capacity qualifications.

Raw records, source/dist archives, package data and the npm observation note
remain private outside worktrees. Do not add them to Git. This public summary
records their limited outcomes and gaps; it is not their raw replacement.

## Scope still open

B and its timer fix remain unmerged. Backend Rust, a real backend for the
skipped parent, browser interaction/rendering and actual dashboard delivery
are NOT_RUN on this composition. No SSO, worker/runtime, provider, cluster,
production gate or release is qualified. Public code and unrelated WIP acquire
no test result by this documentation update.
