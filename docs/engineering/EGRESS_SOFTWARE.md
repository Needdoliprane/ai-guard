# Egress software contracts

[Français](EGRESS_SOFTWARE.fr.md) · [Approved direction and full requirements](EGRESS_CONTROL.md) · [Validation evidence](evidence/2026-11-13/egress-core-protocol/README.md)

The Rust library now contains `egress` policy, CONNECT parsing and flow-state contracts. They are usable software primitives with targeted tests. They do not expose a proxy listener, a gateway route or a new runtime mode. Existing worker networking remains `none` or explicitly requested `bridge`; bridge traffic is still unmediated. There is no new configuration switch that activates destination filtering.

## Implemented behavior

- `Authority` accepts exact ASCII DNS names with an explicit nonzero port. It normalizes case and rejects URLs, userinfo, wildcards, IP literals and ambiguous numeric forms. Unicode and trailing-dot aliases are refused by this closed profile. Wire authorities are bounded to 512 bytes, canonical names to 253 and labels to 63. Errors do not echo input.
- `EgressPolicyV1` is an immutable, revisioned snapshot. Effective evaluation requires both tenant and session policies to be active and allow the exact name/port/transport. Empty policies deny. Canonical SHA256 digests bind revision, interval, destinations and limits. At most 128 destinations are accepted; limits can be reduced within the policy envelope.
- `ValidatedResolution` validates at most 16 supplied IPv4/IPv6 candidates before copying, rejects the entire mixed set if any candidate is special/local, and sorts/deduplicates accepted candidates. It performs no DNS lookup and proves no resolver identity. The conservative special-range classifier cannot identify a control service using an otherwise public address; contextual exclusions are still required.
- The borrowed `httparse` frontend accepts a closed HTTP/1.1 CONNECT profile with one matching Host, at most 16KiB of headers and 32 fields. CL/TE, authentication headers, ambiguous requests and premature tunnel bytes are refused. Its success response requires a current broker-acceptance transition; this local state does not authenticate that broker.
- Closed payloads use canonical bounded base64 and a fixed-shape preflight before variable payload ownership. Data is at most 16KiB decoded, its frame 24KiB; other frames are at most 2KiB. There is no second length-prefix codec or extension to legacy relay V1/V2.
- The pure flow ledger has 16 slots, four pending opens, monotone IDs, four opens/s with burst 8, deadlines and separate directional credit. Credit plus queued bytes is bounded to 256KiB per direction. HalfClose returns unused credit while retaining queued-byte reservations until drain/drop. Reset discards reservations. These are numeric states, not observed socket operations.

The current capability V1 requires its exact fixed flow limits. Lower or raised negotiated flow limits are refused; configurable runtime profiles are not delivered by this library tranche. Policy limits and flow-negotiation limits are distinct.

## Authority and remaining integration

`PolicyAllowed`, `ValidatedResolution` and a wire `Accepted` are descriptive values. None can mint a durable connection permit, choose a tenant or establish a runtime binding. The future trusted owner must authenticate the exact worker/epoch, persist mandatory audit before connect, pin a validated numeric address and own/close its actual sockets. It must also reserve real framing/JSON/IO buffers, arbitrate STOP fairly and apply revocation/expiry. Fixed logical byte counts are not a measured process-RAM bound.

CONNECT authorizes only opaque TCP transport in the future integrated path. It does not establish actual HTTPS, certificate checks or absence of tunnels. Provider-key custody remains a separate boundary; no key protection against the worker is introduced here. The frontend holds no ai-guard credential.

Store/API policy administration, grants and reserve accounting, trusted permit issuer, common framed-channel negotiation, listener/bootstrap/runtime handoff and dashboard are not wired. No worker Internet activation, sensor, MITM/PKI or cluster action was performed. Direct-attempt visibility, remote Web/MCP coverage, all four provider modes and Docker/Podman/Kubernetes acceptance remain mandatory open requirements.

## Validation and reproduction

The frozen composed source passed 34 selected pure parents: 14 policy, 6 frontend, 11 protocol and 3 existing identity-mapper parents replayed in the same composition. Its all-targets check and formatting passed. The two review defects had first been reproduced as two actual test failures, then corrected and independently rereviewed. Source hashes and exact scope are in the [evidence](evidence/2026-11-13/egress-core-protocol/README.md).

For the 31 egress parents on that source, this is the equivalent Cargo selection inside the repository-approved isolated QA environment:

```console
cargo test --frozen --offline --lib egress:: -- --test-threads=1
```

It is not a runtime, throughput or 100-worker test. No production gate is closed by this software result.
