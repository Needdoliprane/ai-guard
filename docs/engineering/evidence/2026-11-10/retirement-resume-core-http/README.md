# Retirement Resume Core/HTTP — 10 November 2026

The delivered slice accepts a closed trusted-control audit request and validates
its complete HTTP receipt. Three discriminating software REDs precede five exact
FIX PASS and an all-targets check RC0. It does not implement full broker Resume,
original-owner permission, retirement transport, quota release or runtime recovery.

## Exact sources and review

Tested base: public `b6dc5db05173848c18047184ecc71f610b203472`.
RED2365 full manifest:
`6a8a337a20752062c5413bd7111c9f86cae7f858477bd744933024447d94de51`.
FIX2366 full manifest:
`a055c2896a0558a2992eca709ef70fb190073460c1319454b28e5b754bb2e54d`.
Owned5 manifest:
`9e51ccedc34c32aed816ea3edcbe1debcdc863923cc0f974cf389df341348d97`.
Independent GO_SOURCE review:
`45f4001ff01d834a326c2c651069165fcaaa0ac7257563a6531df85fd36a5957`.

Only audit.rs (98 additions, no removals), runner/cold_audit_http.rs, its new
retirement_resume_http_tests.rs, tests/d5_control_audit.rs child wiring and the
new child retirement_resume.rs change. Core12 remains exact. The final Core2
spare-capacity test strengthening is identical in RED/FIX, child SHA
`f61ec1e55d421ccc8645605cb0fe2d6065b880db334e02b07d795d2206f5b2a5`.

Publication is prepared on public
`81accef089d9cf52610005f35c733567f298413d`: all five preimages are unchanged
from b6 and all five postimages equal QA. Public81+five contains2372 files,
not the tested2366; its30 out-of-scope differences are exactly the previously
published Auth/docs changes b6→81. No result is transferred to Root working WIP.
The paired operator-guide additions are a separate documentation delta.

## RED, canary and five FIX parents

On unchanged baseline product, Core1 and Core4 reach403 instead of the required
new trusted200 after their prerequisites. Core2 establishes spare ordinary
capacity, then fails because direct Store append wrongly accepts the new fact.
Those three are the discriminating REDs, each RC101. Core3 already PASSes by
rejecting malformed unknown-kind input; it is a negative canary, not a fourth RED.
The HTTP parent is FIX-only because its real port is absent from RED.

These five exact FIX names each select one test and PASS/RC0:

```text
retirement_resume::retirement_resume_fact_requires_runner_control_route_and_replays_exact_receipt
retirement_resume::retirement_resume_fact_uses_durable_control_reserve_without_ordinary_promotion
retirement_resume::retirement_resume_fact_refuses_noncanonical_or_unknown_request_fields_without_mutation
retirement_resume::retirement_resume_redacts_before_storage_and_refuses_redacted_binding_collisions
runner::cold_audit_http::retirement_resume_tests::retirement_resume_http_requires_the_entire_actual_stored_event
```

The four Core functions use actual Router/SQLite and synthetic request facts;
Core2 checks spare-budget refusal/head/count/readonly quota bytes, then retains
the exhausted-budget/control-reserve/reopen branch. HTTP uses real loopback
Router/SQLite commits before corrupting the response: seq-only, changed UUID or
changed digest all refuse. An expired deadline or redaction collision emits no
additional request. These are controlled software faults, not runtime observations.

| Operation | Actual result |
|---|---|
| First host launch |RC127: bare docker absent from host PATH, before Docker/application execution; preserved.|
| Corrected RED build22727 |RC0,12:43:40–12:47:48UTC, same immutable source/empty target.|
| Four RED-source parents |Three discriminating RC101; one negative canary PASS.|
| FIX build3179 |RC0,13:08:16–13:15:12UTC; Cargo reports6m55, separate source/target.|
| Five FIX parents |Five unique exact PASS, no ignored/empty selection.|
| All-targets check26813 |RC0,13:19:05–13:20:25UTC; warnings retained.|
| Post-FIX source |2366 hashes/pathset exact, nonregular0, OOM events0.|

The actual FIX command repeats DEV/TEST DEBUG=0 assignments with identical
values. Root accepted that equivalent environment and preserved actual argv;
no build was repeated to normalize it. Frozen/offline builds use --no-run;
parents use --exact --nocapture --test-threads=1, four via d5_control_audit and
one via --lib, under Root's TERM/KILL/resource bounds. No application runs on
the maintainer's host, and this publication preparation executes no project code.
Private ROOT_FIX5_INDEX.json SHA:
`65f4b606bd8eb360d7ae9a86582459ea95c2b53cb1405c0a4f89ed6a8d0b0361`.

## Contract and conservation

Exact kind `broker.retirement.resume.requested` uses the existing runner-only
POST `/api/v1/control/events` and fixed control-transport path
`/control/api/v1/control/events`. The flat version1/11-field body has bounded
canonical UUIDs, generation, digests and deadline; command UUID equals Event ID.
Shape/attribution are checked before and after actual redaction. Ordinary Store
append and ordinary ingestion cannot select its control reserve. No new route,
credential, service, schema migration, capacity or free tenant selector is added.

The HTTP sender keeps min(original Instant, inherited45s), capped request120KiB,
response512KiB/depth64 and exact whole-Event comparison with formed storage
metadata. A receipt is data, not a local journal ACK, chain checkpoint, original
owner, named-human instruction or emission permission. The public broker service
is not wired to perform full explicit retirement Resume by this slice.

Actual archives were strictly checked against each full manifest: RED2365 SHA
`3cf27c558de4904044ca44cc9edf5e3bc6161a57209c2efd7db8b1e585c5b425`;
FIX2366 SHA `f36d728fb72dbba66c392d9dea3a03776bc165e35767c82cbdbcb6a398fd5f7b`,
24217600 bytes. All members regular, exact contents/pathsets, no PAX/AppleDouble
or links. Root closure report SHA:
`2f3d5def1051da05c45119540016e3b4492e9dd82d029152f9aaf36ce5ac8344`.
Only idle sleep remained before normal stop/remove at13:26:25UTC. Exit137 is
explicit idle stop, not a test/OOM. Peak7463731200 bytes, scratch3174244352/
4294967296 bytes,29775/131072 inodes; OOM0. Sources/raws remain outside WTs.
Other QAs are excluded from this closure.

No native backend/eight parents or two refused timing fixtures were retried,
reassigned or reformulated. Full Resume orchestration, original-affinity ticket,
STOP/fencing, cold/physical recovery, actual runtime/provider/100-worker tests
and global gates remain outside this result. Clippy and warnings are separate.
**G09 FAIL;0/12 global gates closed.**
