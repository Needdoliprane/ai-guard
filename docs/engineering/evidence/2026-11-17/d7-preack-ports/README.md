# D7 pre-ACK ports — source candidate, 17 November 2026

This is the bounded configuration/client/executor portion of approved option A.
Source commit: `f73e9a4895cd160f5158ced944c39c7d6959ecca` (seven Rust paths).
Follow-up `bc607344a94ddf860ee93715163fb2aad1035a46` also releases the matching
client prospective retention through the existing verified native-join Q port.
Follow-up `7477848815bd1e88671cbd6c247184ad07b31b9d` consumes the Original nonce
before Bind transport, so an unknown reply cannot cause its re-export.
It must be composed with the native admission and STOP-owner implementation.
It is not an independently usable fix. Compilation, all regression bodies and
independent review are **NOT_RUN** for this source. No D7 finding, production
gate or runtime guarantee is closed. [French](README.fr.md).

The private registry V1 and V2 accept `max_recovery_stop_owners`, defaulting to
zero and bounded by 1024, independently of `max_enforcer_slots` (S). W is
propagated at fresh Full3 genesis and to recovery/existing internal roles.
Compatibility wrappers pass W=0. For W>0, one native cell is reserved for
catch-up; at most W-1 additional cells can permanently accept STOP owners.
The declared maximum is S+W contexts, with at most S+W-1 permanently covered
targets. W=0 adds no context or catch-up capability. Availability still depends
on actual native installation and the separate source-pinned evidence.

The prospective target is a distinct strict type without an Arm receipt.
`RegistrationV1` continues to require and validate the real receipt. The
authenticated native response binds epoch, admission generation, cancellation
generation, nonce and immutable target digest. The exact pending Arm is
durable before Reserve. An opaque process capability comes only from the
first authenticated native `NotIssued → MayHaveIssued` exchange transition;
Bind checks its nonce again. A lost reply or replay never recreates it.
Client retention survives a
dropped waiter and an unknown Arm exchange. Binding never changes the Arm
identity, snapshot or deadline. A historical pending Arm can only reconnect to
an existing native owner and receives STOP/ObserveOnly, without another claim.

**Compatibility gap: the default Legacy selection has no prospective native
factory.** The candidate explicitly refuses a fresh Legacy Arm before its HTTP
exchange. Existing acknowledged Legacy targets retain their STOP/recovery
routes. No automatic switch, deletion or migration is performed. A real V1
prospective port or a separately reviewed explicit migration that preserves
the existing data is still required before claiming equivalent delivery of
the default user journey. A fresh Full3 role is an explicit new-role operation,
not a migration recipe for an existing installation.

The executor regression source uses real journal/writer, Gateway, SQLite and
native IPC with synthetic Created metadata and the native SyntheticReady
fixture. `/usr/bin/true` is pinned as invocation data only; no engine or
provider is run. Refusal cases assert zero Arm/claim HTTP requests. Legacy and
cancelled cases keep the journal head unchanged; capacity refusal keeps one
durable pending Arm with the same identity and deadline on retry. The
pending-ACK case checks one idempotent Arm replay,
the original identity/deadline/input, unchanged worker state, and no claim.
Private fixture roots are retained because Writer Drop does not itself prove
its native return.

The writer-failure case pauses the real writer after its Journal call and
injects an unknown acknowledgement before Reserve. A negative client latch
then refuses new identities. A strict read remains UNKNOWN or retains that
single exact pending Arm; retry sends no Arm/claim. The latch does not prove a
durable cross-process recovery transition.

Availability after capacity refusal or unknown pending publication remains
**OPEN**. Pending DATA without a real existing native owner cannot allocate a
new Original on retry, and no quota is released. Controlled recovery needs a
distinct reviewed continuation proving native non-engagement. There is no
hidden intent-owner pool, automatic new Arm or implicit migration. These
limits prevent claiming complete D7 remediation from this candidate.
The dedicated second-Reserve/S=1 assertion after verified Q is not yet present;
existing retirement parents must not be presented as that reuse proof.

Only rustfmt and `git diff --check` have run for this portion (RC0). See
[typed status](TESTS.json). Docker, Podman, Kubernetes, provider calls, resource
measurements, latency and end-to-end qualification remain **NOT_RUN**.
