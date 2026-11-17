# D7: native recovery STOP owners

[Français](D7_STOP_RECOVERY.fr.md). Implementation candidate, 17 November 2026.
Compilation, software regressions and independent review of this candidate
remain **NOT_RUN** until their separately recorded validation. This document
qualifies no engine, physical deadline or production gate.

The broker registry accepts `max_recovery_stop_owners` (`W`, default `0`, range
`0..=1024`) separately from unchanged `max_enforcer_slots` (`S`). The private
enforcer receives `--max-recovery-stop-owners`. W adds STOP resources, never
Original/start capacity. For W>0, one cell is reserved for catch-up before
endpoint publication; W−1 cells can retain additional permanent STOP owners.
Permanent capacity is **S + max(W−1, 0)**, within a total native budget S+W.
S=1/W=2 can own two recovered targets and keep one catch-up cell. S=1/W=1
can own one target and attempt the surplus. W=0 provides no catch-up resource.

An accepted target retains its verified immutable RAM snapshot, original
identity/deadline, latest admitted Cancel, native role lease, actual thread
handle and constructed runtime context. `StopReserved` follows installation;
an unconstructed context receives no active-coverage ACK. Neither an engine
error nor a stopped observation retires an owner. These cells create no new
Arm, Original nonce, Q/R+, eviction or quota release.

STOP selection does not read a discovery catalogue or take a writer/HTTP
mutex. Native executable/endpoint and exact-snapshot checks remain. Blocked
runtime IO retains its cell and handle without unlimited replacements. The
catch-up cell holds at most its in-flight snapshot and one queued snapshot;
its actual runtime future must return before another attempt uses that cell.

Each native cell retains the original first error and the latest positively
settled error, including their typed causes and collected captures. Earlier
settled errors can be omitted; an explicit checked count records those
omissions. A third position retains the actual uncollected call, its original
cause, Child/pipes, partial captures and handles. No error string, task panic,
missing downcast, lost notification or timeout certifies `NoChild` or
`Collected`. Only the real spawn/collection producers issue these opaque
proofs. A cell with pending or unknown IO cannot start another attempt or
join for retirement. An ordinary collected failure permits the same context
to retry, and permits catch-up to move fairly to another target. The catch-up
history retains the exact request beside each retained cause. Besides the
current/queued snapshots, at most two settled-cause target payloads can remain
retained, plus bounded captures. This adds no native cell, thread or runtime
budget.
The single caught panic payload of a prospective/bound native thread is also
retained as its original opaque object; it makes custody explicitly unknown
and does not permit a replacement invocation or Q.

The prospective and bound Full views share the same cause owner. A failed
kill followed by a successful exact stopped observation keeps both facts;
a failed follow-up keeps the original kill cause and follow-up cause
separately. Existing public runtime Result wrappers keep their behavior;
the crate-private native port supplies this richer result to D7 consumers.

A surplus remains `RecoveryFenced/Incomplete`. The preinstalled catch-up cell
performs exact reopen/STOP attempts. The control cache retains verified source
proofs and retries eligible targets in bounded round-robin sweeps. An attempt
gets no active-coverage ACK or retirement; it does not drain an old start.
`recovery_attempts` counts returned attempts, including a check that no stop
was due; `recovery_attempt_failures` records unknown/error results. An engine
error never counts as an observed stop.
`recovery_attempt_in_flight` stays true while its actual native call has not
returned; a transport timeout does not clear that snapshot or reclaim its cell.

Discovery retention has a separate bound of 16,384 distinct targets per STOP
client. Its source proofs and pending requests are DATA, not active owners.
An over-limit, incomplete, divergent or unavailable source leaves recovery
incomplete, with no eviction. Eager reports, serialized transfers, retained
requests and active snapshots all consume memory: this is no constant-memory
guarantee for arbitrary histories. Each cell requests a 1 MiB thread stack,
plus actual runtime, IO, snapshot and transfer resources. Physical memory/FD
and latency limits remain to be measured on the chosen platform.
An S cell reserved prospectively retains its original snapshot and the bound
receipt-bearing registration after binding: at most two snapshot payloads per
S cell, with the same single native thread/runtime, until its real exit.

Authenticated `BeginRecovery` commits the same native admission gate shared
by all Full views of the lease. Cold roles start fenced before ledger scan;
a Full view published later inherits that gate. Register finalization and
Prepare check it again. Existing STOP/Cancel and already authorized settlement
continuations retain their controls. An old daemon unable to acknowledge
this operation is refused, without takeover, daemon kill or inferred migration.
Fresh Legacy business launches are explicitly refused before a new Arm RPC
because that path has no prospective native factory. Its existing STOP targets
retain their recovery path; this compatibility gap is not a completed migration.
The pre-Arm sequence persists the exact Pending request, reserves its native
S owner, then consumes the native one-time Arm-exchange grant before the RPC.
If capacity refuses after Pending but before an owner is installed, the same
identity/deadline remain retained. A retry with a missing native owner refuses
to recreate Original from Pending DATA. Recovery availability for that case
remains an explicit open gap; no synthetic cleanup or quota release closes it.

Status distinguishes S/W, reserved catch-up cells, installed active owners,
fence/incompleteness and returned attempt counts. Transport health and counts
prove neither exhaustive source coverage, stopped containers, old-start drain
nor a qualified deployment. The two-target regressions preserve the cap2
control and use unchanged S=1 with explicit W=2; a W=1 case requires an actual
catch-up attempt and incomplete result with one active owner. Their unavailable
runtime fixture proves no container effect. Results belong to their exact
composed source and executable.

The authenticated native STOP status additionally exposes
`retained_native_causes`, `omitted_settled_causes`, `cause_omissions_exact`
and `native_io_uncollected`. These bounded diagnostics grant no Arm, Q,
terminal or quota right and contain no captured content. Missing diagnostic
fields from an older peer give no exact omission coverage. This extends only
the native transport status; public journal recovery records are unchanged.
Collection and cause regressions use real software Children/pipes and native
ownership with synthetic Ready/receipt metadata where stated. Their sources
remain **NOT_RUN** until Luna validates the frozen composition. Default Legacy
compatibility and missing-owner post-Pending capacity retry remain open with
the recovery criteria described above.
