# Human source adoption and owned startup — software validation

[Français](README.fr.md) · [Sanitized results](RESULTS.json) · [Operator procedure](../../../../setup/human-sources-maintenance/README.md) · [Owned identity/resources](../../../../setup/identity/OWNED_SOURCES.md)

On 13 November 2026, **107 unique named functions pass on the same frozen
source in two distinct campaigns: 34 then 73**. There are no failed or ignored
functions and no accepted omissions. This is targeted software evidence, not a
single run of 107 tests or production qualification. Independent result reviews
cover both campaigns. Named libtest results do not independently establish the
completeness of every internal subcase.

## Source and result accounting

The tested source is `dbee1d5f90abd884badf5cd255b69c436f38ae4a`, tree
`f3f8c1e3ed7d3358526d1cf926d9b99f1b6f5b6b`. Its input archive has 2771 regular
files, 34061824 bytes and SHA-256
`7050d12ba3f8fe621b8efcf91a614c3715f410de2bf9302f07b310af89e79302`.
The documentation baseline `e9c474af68c5ce9d91e963515801e108bd39f93e` has that
source commit as its parent; this report does not transfer the results to an
unidentified checkout.

| Campaign | Group | Named functions PASS |
|---|---|---:|
| combined-r3 | owned-api | 21 |
| combined-r3 | owned-audit | 4 |
| combined-r3 | owned-startup | 4 |
| combined-r3 | owned-config | 4 |
| combined-r3 | owned-http-lifecycle | 1 |
| remaining-r1 | legacy-scoped | 5 |
| remaining-r1 | legacy-migration | 13 |
| remaining-r1 | legacy-context | 10 |
| remaining-r1 | legacy-context-fairness | 6 |
| remaining-r1 | legacy-http-lifecycle | 8 |
| remaining-r1 | bootstrap-cli | 7 |
| remaining-r1 | bootstrap-integration | 6 |
| remaining-r1 | bootstrap-operator | 8 |
| remaining-r1 | bootstrap-preparation | 2 |
| remaining-r1 | shared-registry | 6 |
| remaining-r1 | shared-store-head | 1 |
| remaining-r1 | shared-auth-incarnation | 1 |

The five first groups total 34; the twelve later groups total 73. Their named
sets are disjoint. The later campaign resolves the declared Rust module chains
and matches all twelve fresh selected lists to actual compiler registrations
before functional execution. It does not repeat the first 34 functions.
[RESULTS.json](RESULTS.json) lists each executed name and the reviewed output
hashes, source/capture facts and four retained primary receipt/review hashes.
Private fixtures, credential material, binaries and raw archives are not
published here.

The all-targets check passes in combined-r3. Remaining-r1 explicitly references
that **previous exact-source PASS**, without rerunning the check. Both campaigns
complete test compilation with `--no-run` and identify four real
Cargo artifacts: library tests, main tests, bootstrap integration tests and the
normal `agentsandbox` CLI. The normal CLI's `human-sources` help passes in both.
Remaining-r1 records zero compiler errors and 164 warning diagnostics; strict
Clippy, the full suite and release validation are not run.

## Observed execution and closure

The software runs in isolated Linux ARM64 QA with Rust 1.99.1 and image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
The observed profile uses no network, UID/GID 1000:1000, a read-only root,
non-privileged execution, dropped capabilities and no-new-privileges, four CPUs,
9 GiB RAM without swap, 128 PIDs, 4 GiB scratch and a 1 GiB file-size limit.
This QA container is not a business worker or a qualification of its runtime
adapter.

Each campaign completes seven independent collections with RC 0. Root verifies
all 2771 post-run source payloads and modes against the input. The source
exports are 34263040 bytes; their distinct archive hashes remain in the results.
The real anchored fixture roots are observed empty after RAII teardown, and an
actual GNU tar producer captures each 10240-byte empty archive. These are
observations, not fabricated fixture evidence or durable fixture contents.

Measured QA memory peaks are 9158021120 and 8938557440 bytes under the
9663676416-byte limit, with OOM and OOM-kill counters zero. The observed build
rlibs are 545228954 and 545229866 bytes, above 512 MiB. These are QA build
measurements: they neither measure a business worker's usage or price nor prove
the exact file or signal cause of the older `a1669bf` failure.

Both closures return RC 0 for stop, removal and exact container listing; Root
confirms the exact containers absent. The holder's exit 137 follows deliberate
stop and is not an OOM or product-failure verdict. For remaining-r1, the original
Luna execution handle 25710 is lost after a context reset and reports
`UNKNOWN_PROCESS`; no execution is restarted. Root observes the existing
PID 34620 with Cargo/rustc still live, then verifies terminal receipts and exact
RUN PID/container absence. The later Luna agent only observes completion.
The terminal basis is trusted `FINAL` and every phase RC, actual Cargo/test
summaries and process/container continuity; a unified tool terminal exit is
**not directly observed**.

## Preserved failures and limits

The earlier four-error compilation failure on `3a79c52` reaches no test. The
later no-run build on `a1669bf` stops with signal 25 after an all-targets PASS;
the 512 MiB file-size-limit cause is strongly supported, not formally proved,
and its tests remain `NOT_RUN`. Pre-execution proposal metadata/order and naming
corrections remain preserved separately. Combined-r3 then passes 34 functions
but stops with **orchestration RC 85**: its scoped-read selector uses the wrong
Rust namespace. The fresh-list guard refuses the empty selection. Closure still
returns RC 0; that campaign is not rewritten as a globally successful run.
Remaining-r1 corrects the harness selector, with no application patch, and
passes the remaining 73. Its own final/closure records return RC 0.

The operator path prepares existing Store13/Auth4 sources, invokes the real
union migration to Store14/Auth5 and publishes activation; it creates no named
person, credential or grant. `--confirm-control-stopped` remains a manual
attestation, without fencing an older consumer. `check` checks private
files/metadata only. Interrupted operations retain the same sources, IDs,
configuration, journal and backups for the documented resume step.

The owned profile deliberately adopts only the human identity/resources GETs.
`contexts_complete=false` describes unknown memberships when another tenant
source is skipped; a single-tenant union can report complete contexts. Known
unadopted routes refuse with 503 before legacy/body/Store work; unknown URI or
method keeps normal 404/405 behavior. Selected OIDC remains in the source union
with its known handlers closed, including status/CLI-profile. Only when OIDC is
absent do its existing disabled-state status/CLI-profile GETs remain admitted.
`/healthz` GET/HEAD reads memory and does not prove source readiness.

This result does not qualify physical power loss, native macOS, business
workers or their real Docker/Podman/Kubernetes adapters, real IdP/provider,
frontend/browser, strict Clippy, full suite, release or 100 workers. Modern OIDC
sessions/revocation, human approval/launch, egress permission/filtering and
modern identity provisioning remain mandatory open work. The default legacy
mode is unchanged. No audit finding or G01–G12 production gate is closed.
