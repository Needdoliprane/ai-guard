# Relay owner retirement — local Full3 candidate

This evidence concerns unpublished Full3 broker-owner source preserved in local
seed `f1ce30456bdf7cf46fe9c8139cd88fbe8d22182c`, parent `ed37805`, plus the
reviewed R2 correction. It does not make that seed a public or qualified release.

The correction retains Unknown/error until a compatible durable acknowledgement,
publishes a candidate only after save, and rereads the journal after interrupted
IO. It accepts the three explicit Core terminal states only with the required
local join. Independent review found no blocking source defect. Four Rust
postimages were normalized with rustfmt; both canonical normalizations and the
reviewed preimages were checked. The four original regression bodies did not
change. No engine, business child, CLI provider or model call was used.

## Observed validation

- Baseline r1: three classified unit REDs; a fourth failed in fixture setup.
  Its experiment remains INCOMPLETE, not a fourth product RED.
- Baseline r2: corrected journal fixture reached the expected RC101/Scope oracle
  after valid Reserved/reopen and Closed controls. No earlier RED was repeated.
- GREEN: **six distinct tests PASS**, zero failed/ignored, at13:22:42–43 UTC.
  Four cover status/error/acknowledgement/journal; two use actual journal IO,
  capacity admission, observer cancellation, join and durable reread.
- Build and `cargo check --frozen --offline --all-targets`: RC0; check completed
  at13:24:59 UTC. Independent historical compilation overlapped this check.
- Post-source verification, actual pre-stop export, stop/remove and exact-ID
  absence check: RC0. No OOM. The container was removed.

Frozen 2,517-file archive SHA-256:
`5ee9fe53ccc36b037a34aef73527c920a576affabf14f4182fcf0958e5e932fa`.
Manifest:
`085651a3b5d1c78e63977e17c964dbe01d7dace15823114a06919e3c9244a49e`.
Genuine post-run archive: 28,241,920 bytes, SHA-256
`e3f0fe1986fa1a6a7a7a9c73a244a1437f36c4559308d170886d7629846ef872`;
every payload matches that manifest. Raw private evidence is retained separately.

The same isolated LinuxARM64/Rust1.99.1 profile as the
[historical Core validation](../relay-history-reconciliation/README.md) was used.
One offline lib-test build resolved the six exact parents; each direct execution
had a60s bound, followed by the all-targets check. No broad suite or provider
matrix was repeated.

## Limits

IO-capacity refusal is not a filesystem/fsync failure. The cancellation test's
terminal acknowledgement is synthetic, not an independent Core HTTP receipt.
Declared Worker terminal/join preconditions do not qualify the whole job wrapper,
process death, cold restart or a real worker. A real ACK followed by a refused
local save still needs its full integration proof.

Historical broker consumption, full-prefix V1/V2 history and runtime qualification
remain open. The source review does not close the primary audits by association.
See the [recovery register](../../../RELAY_RECOVERY_REVIEW_20261111.md).
