# Relay consumers on the current composition — 12 November 2026

Two exact software regression parents and the all-targets compile check pass on public source c173086 plus the 134 frozen private Full3 paths and the previously reviewed test-only counter overlay. Root independently verifies the actual post-run source and raw results. The Full3 application changes remain private; this evidence publishes neither those changes nor a runtime qualification.

## Source and actual results

- Source archive: 2,643 regular files, all mode 0644, 31,447,040 bytes, SHA-256 0a0d7a73d29a25e33d8f8677704537fc8ad07cefbcda030a073161cb935bc9dd.
- Base: c173086e53f734e3e1988e5468725858ee90720b. The 134-path set is frozen before execution. Its retirement history test is replaced only by the frozen 65959b3fc91453877015a1f77540487b66e4ad00c05d669da359796fe3510147 overlay.
- A single offline lib-test build and actual listing pass. Both exact reserve/claim acknowledgement-loss consumers report one passed, zero failed and zero ignored. cargo check --frozen --offline --all-targets then passes. Names and raw oracle hashes are in [TESTS.json](TESTS.json).
- Exported live source: 31,631,360 bytes, SHA-256 bc6215556757255a7e83b10f07537274a3965b6c43f2572a64d08bdca2570d9f. Root parses USTAR as data without extracting it on the host and compares all 2,643 payloads/modes. The separate overflow sentinel is empty.
- Root hashes and verifies 171 runtime artifacts, manifest SHA-256 a3e0ae67998215fd4294a0671f3376188a29cba94f13e66621b25342093b2b0f. Independent results receipt SHA-256 ad53c0c107ebde117ee66693dbdb0270807b5ce94dbaf29645a927b50ab754df.
- Pipeline and closure return0. Exact container stop/remove and a fresh exact-ID inventory establish removal. Memory peak is 7,475,396,608 of 9,663,676,416 bytes; oom/oom_kill/max counters are0. Exit137 belongs to the controlled sleep-holder stop, after target commands completed.

## Execution and evidence history

The admitted QA runs Linux ARM64/Rust1.99.1 in pinned image sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207: UID 1000, no network, read-only root, dropped capabilities, no-new-privileges, kernel seccomp 2, private IPC/cgroup, four CPUs, 9 GiB memory maximum and no container swap,128 PIDs,1024 FDs,4 GiB scratch. Only frozen inputs and the public registry cache are mounted read-only. No project code executes natively on the host. Independent migration compilation overlapped; these timings are not product latency measurements.

The R4 archive/inputs/harness were independently admitted before CREATE and verified again in the created, non-running container before one Luna RUN. Cargo JSON identifies the sole actual library artifact; the built listing selects both parents exactly. Build/list/parent/check bounds are1200/30/30/900 seconds. The EXIT path verifies source, caps its export at32 MiB plus an overflow byte, collects metrics and closes the exact CID.

Earlier packages remain separate: R1 was statically rejected for harness deficiencies; R2 was superseded before execution; R3 failed static pin destination mapping. None ran target code or contributes a PASS.

After R4 closed, its operator handoff report was edited. The preserved 35-entry static manifest had passed Root admission before execution, but now has one mismatch: HANDOFF.md changed from 6420d02615bef4d6d3896894d10ff25f20873694d947be427bd8a7d6810b7780 to 78c8b8d92ee7db456237bb5e20c2d258fdbf182905b921b1abf202b228ae282e. All other 34 entries, including source, scripts, selectors and pins, remain exact. The complete static manifest is not reported as passing after that edit. No old report was reconstructed and no test was rerun to hide this traceability defect. The runtime/source oracles above were independently checked.

Private raw evidence remains in eng/qa-full3-current-composition-20261112-r4 under the private engineering archive; the duplicated eng path component is part of its actual location. The source, original manifest, current report, Root receipts and post-run mutation record are retained.

## Limits

This exercises two consumer corrections using real Rust journal/SQLite/control fixtures, without a real worker engine. It does not qualify all 134private paths, complete broker fencing/restart/retirement, process death or runtime joins, the complete relay transport,100 workers, multi-host federation, Docker/Podman/Kubernetes workers, suppliers or production. Native enforcer and audit_namespace_guard operations excluded after earlier tooling refusals were not retried or rerouted. Historical tests and the [earlier nineteen-parent correction](../../2026-11-11/relay-history-owner/README.md) keep their own source identities. No global gate or primary audit fingerprint is closed here.
