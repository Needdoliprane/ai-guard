# Claude A/B composition — 13 November 2026

[Français](README.fr.md) · [Typed results](TESTS.json)

**Source-specific software targets passed, with an explicit subcase gap. The global harness returned 99; this is not a global PASS.** Luna executed one admitted campaign in a Linux ARM64 QA tool container. The container image was inspected; no product control/worker image was built or qualified.

## Source and scope

The tested immutable commit is `f12a46af435a09c754084f3a1eb3db9f650f800f`, tree `787590a570920a742996559a7ea395ec6737fb49`: 2,738 files, input manifest SHA-256 `3056861835293268fe182b266b62eafce90a705eb304b0325359adb04e1a941d`. Kubernetes component A is `356312ff1f52d231f980c412643b96e73ab245f2` (24 paths); release component B is `5e8e1811cf7828b12b8a7f94820070606f19f332` (22 paths). All 46 delivered postimages match before the four current guide status updates. Broker source `223b699e959964bef4460f34857516cb8cdd37c8` versus public Root `6cc6b3a25f4dd1fea4ead3180cde1d58fb912001` differs on eight documentation/evidence paths only. Separately, the tested joint source `f12a46af435a09c754084f3a1eb3db9f650f800f` includes the 46 A/B delivered postimages and differs from Root on 54 paths, including 40 outside docs. The intended publication composes Root plus those 46 A/B postimages and the 15 reviewed documentation payloads; Root must independently verify the staged non-document composition before commit. No whole-tree equality between the tested source and a future publication is claimed.

## Observed results

| Check | Observed result |
|---|---|
| Compilation | PASS, RC0, zero compiler errors, 275 warnings |
| 27 unique selected groups | 272 distinct active parents PASS: 144 Kubernetes, 123 release, 5 broker; zero failed; two ignored helpers remain NOT_RUN |
| Fresh lists and per-group verdicts | 27 lists and 27 verdicts, all RC0 |
| All-targets check | PASS, RC0, zero compiler errors, 286 warnings |
| Global harness | RC99; complete subcase coverage not established |
| `it-manifest`: private parent owned by another UID | NOT_RUN: the required second account was unavailable; a passing parent does not cover this subcase |
| Clippy | NOT_RUN |

The full group counts appear in [TESTS.json](TESTS.json). Release manifest/verify are delivered W1–W3 software, exercised on real synthetic filesystem inputs and CLI processes. This is not a real release build, signed provenance or reproducible-build proof. PW1/PW2/PW3/PW6/PW8 remain proposed and unapplied. Image labels remain `declared`.

## Preservation and closure

All 2,738 post-run contents and modes match their expected source entries exactly (post manifest SHA-256 `66c70f708e3741fe685d6f1c4eda1b9fe41b94e2b1f82eeb10ca4b148521b41c`, 33,832,960 bytes). This post-run manifest is a separate serialization from the input manifest, not an assertion that their manifest bytes are equal. The 950-record raw evidence manifest has SHA-256 `f0a35767e2bc46157e961c225cde80aedcf7edf2964bb7574262e593862f1098`. Six collections and closure returned RC0; the exact QA container was removed, OOM counts were zero and temporary storage was empty. Private fixtures and raw traces are retained privately, excluded from publication. The independently checked receipt SHA-256 is `572cdb995aaffa89229b0bce188fc4f29e9348cd1619414a22da79b18c3c1058`.

## Limits and historical records

No real business worker/runtime, provider, IdP, cloud, cluster, native macOS, capacity100, product image or release build was exercised. S5 production effects remain closed; Claude owns active S2b attach, which is not integrated here. Historical Q1/Q6 discussions in the Kubernetes guide remain historical questions, distinct from the newer dispatch; no Claude work is prematurely accepted. Full broker recovery/fencing, runtime relay and production gates remain open. No audit fingerprint is closed.

The original [Kubernetes delivery record](../../2026-11-12/kubernetes-v2-runtime/README.md) and [release delivery record](../../2026-11-12/release-integrity/README.md) retain their original dated NOT_RUN scope. Current guides: [Kubernetes V2](../../../../setup/kubernetes-runtime/README.md) and [release metadata](../../../../setup/release/README.md).
