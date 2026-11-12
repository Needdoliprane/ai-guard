# Operator journey in the English and French README

The two repository entry points now connect prerequisites, private configuration, immutable-source builds, the passive diagnostic, an authorized network-disabled command, dashboard observations, audit verification, session reconciliation, a new export destination and controlled shutdown/restart. They link the existing provider, local identity, SSO/Keycloak, local-inference/TLS, enterprise and Kubernetes guides. Kubernetes worker launch remains unavailable; the engineering state retains delivery and qualification limits.

The change follows the composed operator-diagnostics publication `3f1aeeb2827cd9dd910ea8cf47b3f0ea7796281e`. It preserves that diagnostic introduction in the larger journey. This is documentation review, not an executed installation recipe or a transfer of the diagnostic software test results.

## Static verification

| Check | Observed result |
|---|---|
| English and French prose | Both complete texts independently read; matching scope, limitations and operator steps |
| Relative links | 66 occurrences resolve against the composed public files |
| Anchors | 10 anchor occurrences resolve to existing headings or explicit IDs |
| Command declarations | 15 command occurrences per language, matched to 14 exact source/guide files in the tested E composition |
| Shell examples | Eight byte-identical blocks across languages |
| Existing diagnostic guide test | Its source reads `docs/setup/diagnostics`, unchanged by this README edit |
| Whitespace check | `git diff --check` RC 0 before publication |
| Project execution for this documentation change | NOT_RUN; no build, runtime, provider, network, IdP or E2E rerun |

The private source delivery includes separate patches from public `862e182` and from the final E README preimages. Root used the final E preimages and verified the two replacement payloads, so the diagnostic documentation was not lost during integration. The delivery, link/command/equivalence reports and Root review remain privately retained.

| Artifact | SHA-256 |
|---|---|
| English README | `49ddad308b3b25cbd94344ca6731f020a89ebd60ea5f4f289a20a503948225df` |
| French README | `49149e9e6caf113a2e89619b0ba0315b3f96a05baf59e1adaa58df46e98b2558` |
| Source delivery manifest | `e23a8c1b6c287a8f863b2811a64521d9954eeb3036d15fe6bcd7a475e51802b1` |
| Root independent review | `108b0aac46b11a1ded2a837c88feb784622c8cf51b3a251fe81e52fb654db9a2` |

Read the [English entry](../../../../../README.md), [French entry](../../../../../README.fr.md), [local guide](../../../../setup/local/README.md) and [validation record](../../../VALIDATION.md). No production gate is closed by this change.
