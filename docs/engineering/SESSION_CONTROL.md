# Session creation and intervention — required product contract

Approved by the maintainer on 15 November 2026. **Target to implement and qualify; not a statement of availability.** [French version](SESSION_CONTROL.fr.md). This contract extends [identity and SSO](../sso.md), [UI](../ui.md) and the [architecture contract](ARCHITECTURE_CONTRACT.md).

## One coherent user journey

The authenticated dashboard must let a permitted user create an AI session, then send a short instruction to that same session. Local named identity and optional OIDC use the same server-side authorization and attribution rules. Both capabilities form one delivery branch and one worktree. Codex owns composition, tests with Luna, integration, push and worktree closure; Claude may implement the delegated code and EN/FR guides.

Creation selects an opaque project/source target configured by the trusted control plane, an allowed agent/authentication mode and an initial task. The server resolves and shows the effective provider, subscription/API/internal mode, model, permissions, resources and network coverage. The browser never supplies arbitrary host paths, engine sockets, images, argv, environment or credentials. Subscription selection never silently becomes a paid API call.

Preparation is held: it does not start an agent or consume a start claim. Launch commits the exact accepted prepared intent after current authorization, quota, policy and durable audit checks. Source or configuration changes invalidate the preparation. A lost response is reconciled using the same operation, not a second Submit. Accepted, start issued and runtime-observed running are separate facts.

## Authority and attribution

The authenticated identity determines the tenant. Require explicit, distinct LaunchSession and SendSessionMessage permissions and resource ACLs. Content/read/admin visibility, tenant membership and ApproveLaunch do not imply either mutation. Revalidate membership, OIDC ceilings, revocation and resource scope immediately before the effect. The server creates the real named session grant through the verified existing grant producer; owner/principal fields from browser JSON are never identity proofs.

Keep initiator, effective broker actor and causality distinct. A normal instruction does not approve an action, extend a lease, change a policy, lift a denial or bypass required review. No runner credential enters the worker or frontend, and no human SSO token enters the worker. The browser keeps only its human bearer in memory according to the SSO contract. Existing owned read profiles and closed route/format contracts stay closed; new mutation profiles, provisioning and migrations must be explicit, backed up and coordinated with the integrator.

## A real conversation channel

[Protocol and test design note in French](evidence/2026-11-25/provider-conversation.fr.md).

Create a retained conversation binding during launch, linked to tenant/source, session, broker and incarnation, worker/attempt, runtime instance, input epoch, provider/protocol and expiry. Routing from a declared provider-session log or an arbitrary session UUID is insufficient. Restart, attempt or owner changes never silently retarget a message.

Use documented bidirectional Codex/Claude protocols in a separately resolved conversation mode. Historical batch and TTY sessions remain explicitly unsupported for input unless a genuine retained channel proves otherwise. Do not inject terminal bytes or run a command as a substitute for conversation input. Preserve the four required provider/authentication combinations and qualify each separately.

Keep execution17 network None and the approved [Rust-controlled egress direction](EGRESS_CONTROL.md). A conversation adapter does not enable Bridge, loosen a historical preset or silently fall back to legacy. Reuse the broker's actual egress/identity/fencing capabilities; unresolved prerequisites remain explicit and do not redefine the target as a permanently disabled form.

## Durability, bounded work and truthful status

Use stable idempotency keys. Same operation and canonical content returns its retained result only while the caller remains authorized; different content conflicts. Queue state, named attribution and mandatory audit are durable before start or the first message byte. Audit failure means no effect. Preserve tenant isolation, bounded per-session/principal/tenant capacities, retention and deadlines without consuming the reserves needed to stop workers.

Message bodies are protected tenant business data with content ACLs; the audit records metadata and receipts without duplicating the body. Apply existing secret masking before persistence/display, and do not silently change the instruction. If the approved masking policy cannot preserve its meaning safely, reject it with a fixed reason before enqueue. Never expose raw input in URLs, argv, technical logs or errors.

Queued means durably recorded; transmitted means a complete frame was written to the exact owned channel; acknowledged requires an exact correlated official protocol response. None means that the instruction was executed. Show unsupported, rejected, expired, disconnected and delivery unknown distinctly. After possible emission with no reconcilable acknowledgement, retain uncertainty and do not resend automatically. Stop, cancellation, revocation and the original deadline keep priority; a message does not restart the worker or reset its quota/time budget.

## UI and completion evidence

Provide New session and Send instruction in the real authenticated views, driven by Rust capabilities. Keep OVPT editorial direction, both themes, keyboard/focus, responsive layout, explicit destination and provenance. Prevent duplicate activation and late responses from repopulating revoked contexts. Aborting a browser request does not cancel an accepted server operation. Existing read-only users remain read-only.

Deliver the usable Rust/TypeScript implementation, typed APIs, configurations, migrations and aligned EN/FR operator guides. Test real Store/API/broker and controlled stdio behavior, cross-tenant and same-tenant authorization, idempotence, audit exhaustion, lost responses, stop/revocation races and context clearing. Then qualify the actual runtimes and the provider/authentication paths with economical bounded calls. Fixtures and source reviews do not qualify CLI reception, workers, OIDC or production. Track unresolved dependencies and evidence in the existing engineering registers.
