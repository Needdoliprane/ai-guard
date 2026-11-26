//! SendSessionMessage control plane on the owned sources (Store14/15), under
//! the same validated `session_prepare_v1` capability marker: the conversation
//! and message tables exist exactly when the marker does.
//!
//! The enqueue is a durable, nominative, audited record of one short
//! instruction. The body is protected tenant business data: it lives in the
//! tenant store under the session's resource ACL, never in the audit chain,
//! which retains metadata and receipts only. The trusted conversation adapter
//! later issues the emission (the durable latch before the first frame byte),
//! observes the complete write and reconciles the exact correlated protocol
//! acknowledgement. Each fact is a distinct durable event; delivery uncertainty
//! is retained, never silently resent. STOP closes the right to emit first.
use super::*;
use crate::control::session_control::{
    BindSessionConversationV1, CloseSessionConversationV1, IssueSessionMessageEmissionV1,
    SessionConversationBindingV1, SessionConversationStatusV1, SessionConversationViewV1,
    SessionMessageAckV1, SessionMessageEmissionV1, SessionMessageReceiptV1,
    SessionMessageRequestV1, SessionMessageStatusV1,
};
use crate::identity::{
    AuthorizedScopeV1, ContextModeV1, HumanCapabilityV1, IdentityFailureV1, MembershipV1,
    ResourceGrantV1, ResourceKindV1, ResourceRefV1,
};
use uuid::Uuid;

pub(crate) const ENQUEUED_KIND: &str = "control.session_message.enqueued";
pub(crate) const EMISSION_ISSUED_KIND: &str = "control.session_message.emission_issued";
pub(crate) const TRANSMITTED_KIND: &str = "control.session_message.transmitted";
pub(crate) const ACKNOWLEDGED_KIND: &str = "control.session_message.acknowledged";
pub(crate) const CONVERSATION_BOUND_KIND: &str = "control.session_message.conversation_bound";
pub(crate) const CONVERSATION_CLOSED_KIND: &str = "control.session_message.conversation_closed";

const CONVERSATIONS_TABLE: &str = "session_conversations";
const MESSAGES_TABLE: &str = "session_messages";

const CONVERSATIONS_SCHEMA: &str = "CREATE TABLE session_conversations(session_id TEXT PRIMARY KEY,binding_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL)";
const MESSAGES_SCHEMA: &str = "CREATE TABLE session_messages(operation_id TEXT PRIMARY KEY,session_id TEXT NOT NULL,input_hash TEXT NOT NULL,content_hash TEXT NOT NULL,principal_key TEXT NOT NULL,membership_id TEXT NOT NULL,organization_scope TEXT NOT NULL,binding_fingerprint TEXT NOT NULL,input_epoch INTEGER NOT NULL,content TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('queued','transmitted','acknowledged','unknown')),event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL,receipt_json TEXT NOT NULL)";

pub(crate) const SCHEMA: [(&str, &str, &str); 2] = [
    ("table", CONVERSATIONS_TABLE, CONVERSATIONS_SCHEMA),
    ("table", MESSAGES_TABLE, MESSAGES_SCHEMA),
];
pub(crate) const AUTO_INDEX_COUNTS: [(&str, u32); 2] =
    [(CONVERSATIONS_TABLE, 1), (MESSAGES_TABLE, 1)];

/// Bounded unresolved work: a message occupies capacity until its delivery is
/// acknowledged. The bounds never consume the audit reserves protected for
/// control and identity effects (the stop path).
const MAX_UNRESOLVED_PER_SESSION: i64 = 32;
const MAX_UNRESOLVED_PER_PRINCIPAL: i64 = 128;

/// Fixed refusal reasons of the message control plane, distinct from identity
/// failures so the API can name the exact closed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionMessageFailureV1 {
    /// The binding carries no resolved channel: input is explicitly
    /// unsupported, never a silent legacy or TTY fallback.
    ChannelNotAdmitted,
    /// STOP (or an explicit close) removed the right to emit.
    SessionClosed,
    /// The binding changed since the caller pinned it (restart, attempt,
    /// channel or epoch): no silent retarget.
    BindingChanged,
    /// The original preparation deadline passed; it keeps priority.
    BindingExpired,
    /// An emission was already issued for this operation: delivery
    /// uncertainty is retained and never automatically resent.
    EmissionAlreadyIssued,
    /// The approved masking policy cannot preserve the instruction's meaning
    /// safely; it is refused before enqueue, never silently altered.
    ContentRequiresRedaction,
    /// The bounded per-session or per-principal unresolved queue is full.
    CapacityExhausted,
}
impl std::fmt::Display for SessionMessageFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session message refused: {self:?}")
    }
}
impl std::error::Error for SessionMessageFailureV1 {}

fn failure(error: IdentityFailureV1) -> anyhow::Error {
    error.into()
}

/// Create the message tables inside the held capability-install transaction.
pub(crate) fn install_schema(tx: &Transaction<'_>) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    Ok(())
}

/// Schema validation: the tables exist exactly when the validated capability
/// marker does. Checked by `session_prepare::validate_capability`.
pub(crate) fn tables_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [CONVERSATIONS_TABLE],
        |row| row.get(0),
    )? && db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [MESSAGES_TABLE],
        |row| row.get(0),
    )?)
}

// --- closed audit payloads (metadata and receipts only, never the body) ---

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Enqueued {
    version: u32,
    operation_id: String,
    principal_key: String,
    membership_id: String,
    organization_scope: String,
    session_id: String,
    input_hash: String,
    content_hash: String,
    binding_fingerprint: String,
    input_epoch: u64,
    content_bytes: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmissionIssued {
    version: u32,
    operation_id: String,
    session_id: String,
    principal_key: String,
    binding_fingerprint: String,
    input_epoch: u64,
    content_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transmitted {
    version: u32,
    operation_id: String,
    session_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acknowledged {
    version: u32,
    operation_id: String,
    session_id: String,
    ack: SessionMessageAckV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationChanged {
    version: u32,
    operation_id: String,
    session_id: String,
    input_hash: String,
    binding: SessionConversationBindingV1,
}

/// Replay validation of one `control.session_message.*` event inside the
/// closed control replay. Every record is closed and bound to its session.
pub(crate) fn validate_replay_event(db: &Connection, stored: &StoredEvent) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native",
        "unexpected session message event provenance"
    );
    let canonical = crate::control::session_control::canonical_uuid;
    let session_bound = |session_id: &str| -> anyhow::Result<()> {
        anyhow::ensure!(
            canonical(session_id).is_ok() && stored.event.session_id.as_deref() == Some(session_id),
            "invalid session message session binding"
        );
        Ok(())
    };
    match stored.event.kind.as_str() {
        ENQUEUED_KIND => {
            let record: Enqueued = serde_json::from_value(stored.event.data.clone())?;
            session_bound(&record.session_id)?;
            anyhow::ensure!(
                record.version == 1
                    && canonical(&record.operation_id).is_ok()
                    && !record.principal_key.is_empty()
                    && !record.membership_id.is_empty()
                    && !record.organization_scope.is_empty()
                    && crate::control::hash(&record.input_hash).is_ok()
                    && crate::control::hash(&record.content_hash).is_ok()
                    && crate::control::hash(&record.binding_fingerprint).is_ok()
                    && record.content_bytes > 0
                    && record.content_bytes
                        <= crate::control::session_control::SESSION_MESSAGE_MAX_CONTENT_BYTES
                            as u64,
                "invalid session message enqueue record"
            );
        }
        EMISSION_ISSUED_KIND => {
            let record: EmissionIssued = serde_json::from_value(stored.event.data.clone())?;
            session_bound(&record.session_id)?;
            anyhow::ensure!(
                record.version == 1
                    && canonical(&record.operation_id).is_ok()
                    && !record.principal_key.is_empty()
                    && crate::control::hash(&record.binding_fingerprint).is_ok()
                    && crate::control::hash(&record.content_hash).is_ok(),
                "invalid session message emission record"
            );
        }
        TRANSMITTED_KIND => {
            let record: Transmitted = serde_json::from_value(stored.event.data.clone())?;
            session_bound(&record.session_id)?;
            anyhow::ensure!(
                record.version == 1 && canonical(&record.operation_id).is_ok(),
                "invalid session message transmission record"
            );
        }
        ACKNOWLEDGED_KIND => {
            let record: Acknowledged = serde_json::from_value(stored.event.data.clone())?;
            session_bound(&record.session_id)?;
            anyhow::ensure!(
                record.version == 1
                    && canonical(&record.operation_id).is_ok()
                    && !record.ack.turn_id.is_empty()
                    && record.ack.turn_id.len() <= 128,
                "invalid session message acknowledgement record"
            );
        }
        CONVERSATION_BOUND_KIND | CONVERSATION_CLOSED_KIND => {
            let record: ConversationChanged = serde_json::from_value(stored.event.data.clone())?;
            session_bound(&record.session_id)?;
            let expected = if stored.event.kind == CONVERSATION_BOUND_KIND {
                SessionConversationStatusV1::Active
            } else {
                SessionConversationStatusV1::Closed
            };
            anyhow::ensure!(
                record.version == 1
                    && canonical(&record.operation_id).is_ok()
                    && crate::control::hash(&record.input_hash).is_ok()
                    && record.binding.version == 1
                    && record.binding.session_id == record.session_id
                    && record.binding.status == expected
                    && chrono::DateTime::parse_from_rfc3339(&record.binding.expires_at).is_ok(),
                "invalid session conversation record"
            );
            let _ = db;
        }
        _ => anyhow::bail!("unknown session message event kind"),
    }
    Ok(())
}

// --- rows ---

struct MessageRow {
    session_id: String,
    input_hash: String,
    content_hash: String,
    principal_key: String,
    membership_id: String,
    organization_scope: String,
    binding_fingerprint: String,
    input_epoch: i64,
    content: String,
    status: String,
    event_seq: i64,
    event_hash: String,
    receipt_json: String,
}

fn message_row(tx: &Connection, operation_id: &str) -> anyhow::Result<Option<MessageRow>> {
    Ok(tx
        .query_row(
            "SELECT session_id,input_hash,content_hash,principal_key,membership_id,organization_scope,binding_fingerprint,input_epoch,content,status,event_seq,event_hash,receipt_json FROM session_messages WHERE operation_id=?1",
            [operation_id],
            |r| {
                Ok(MessageRow {
                    session_id: r.get(0)?,
                    input_hash: r.get(1)?,
                    content_hash: r.get(2)?,
                    principal_key: r.get(3)?,
                    membership_id: r.get(4)?,
                    organization_scope: r.get(5)?,
                    binding_fingerprint: r.get(6)?,
                    input_epoch: r.get(7)?,
                    content: r.get(8)?,
                    status: r.get(9)?,
                    event_seq: r.get(10)?,
                    event_hash: r.get(11)?,
                    receipt_json: r.get(12)?,
                })
            },
        )
        .optional()?)
}

fn conversation_row(
    tx: &Connection,
    session_id: &str,
) -> anyhow::Result<Option<SessionConversationBindingV1>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT binding_json FROM session_conversations WHERE session_id=?1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(Into::into))
        .transpose()
}

fn status_str(status: SessionMessageStatusV1) -> &'static str {
    match status {
        SessionMessageStatusV1::Queued => "queued",
        SessionMessageStatusV1::Transmitted => "transmitted",
        SessionMessageStatusV1::Acknowledged => "acknowledged",
        SessionMessageStatusV1::Unknown => "unknown",
    }
}

fn stored_event(tx: &Connection, seq: i64) -> anyhow::Result<StoredEvent> {
    Ok(tx.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [seq],
        row_event,
    )?)
}

fn event_by_id(tx: &Connection, id: &str) -> anyhow::Result<Option<StoredEvent>> {
    Ok(tx
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [id],
            row_event,
        )
        .optional()?)
}

fn message_event_id(domain: &[u8], operation_id: Uuid) -> String {
    super::session_prepare::bound_event_id(domain, operation_id)
}

fn receipt_with(
    row: &MessageRow,
    status: SessionMessageStatusV1,
    ack: Option<SessionMessageAckV1>,
    replayed: bool,
) -> anyhow::Result<SessionMessageReceiptV1> {
    let mut receipt: SessionMessageReceiptV1 = serde_json::from_str(&row.receipt_json)?;
    receipt.status = status;
    receipt.ack = ack;
    receipt.replayed = replayed;
    Ok(receipt)
}

/// Persist one status transition on the retained receipt and row together.
fn update_status(
    tx: &Transaction<'_>,
    operation_id: &str,
    row: &MessageRow,
    status: SessionMessageStatusV1,
    ack: Option<SessionMessageAckV1>,
) -> anyhow::Result<SessionMessageReceiptV1> {
    let receipt = receipt_with(row, status, ack, false)?;
    tx.execute(
        "UPDATE session_messages SET status=?1,receipt_json=?2 WHERE operation_id=?3",
        params![
            status_str(status),
            serde_json::to_string(&receipt)?,
            operation_id
        ],
    )?;
    Ok(receipt)
}

// --- authorization ---

/// Revalidate the caller's personal message context inside the transaction.
/// SendSessionMessage is distinct: read, administration, LaunchSession and
/// ApproveLaunch never imply it.
fn authorize_send(
    tx: &CheckedTransaction<'_>,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    if !super::session_prepare::installed(tx)? {
        return Err(failure(IdentityFailureV1::MigrationRequired));
    }
    let current = identity::revalidate_scope_checked(tx, scope, tenant)?;
    if current.mode() != ContextModeV1::Personal
        || !current.principal().is_human()
        || !current.has_capability(HumanCapabilityV1::SendSessionMessage)
    {
        return Err(failure(IdentityFailureV1::PermissionDenied));
    }
    Ok(current)
}

/// The session's resource ACL for sending: the active nominative grant must
/// name the caller as owner. Shares carry read permissions only; a shared
/// reader never gains the mutation.
fn require_session_owner(
    tx: &Connection,
    current: &AuthorizedScopeV1,
    session_id: &str,
) -> anyhow::Result<ResourceGrantV1> {
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: session_id.into(),
    };
    let grant: ResourceGrantV1 = identity::json_row(
        tx,
        "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
        &resource.key()?,
    )?
    .ok_or(failure(IdentityFailureV1::NotFound))?;
    anyhow::ensure!(
        grant.active
            && grant.resource == resource
            && grant.tenant_id == current.tenant_id()
            && grant.organization_scope == current.organization_scope()
            && grant.owner == *current.principal(),
        failure(IdentityFailureV1::PermissionDenied)
    );
    Ok(grant)
}

/// The binding checks shared by enqueue and the effect point, in priority
/// order: STOP, then the original deadline, then the resolved channel.
fn require_emittable_binding(
    binding: &SessionConversationBindingV1,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    if binding.status == SessionConversationStatusV1::Closed {
        return Err(SessionMessageFailureV1::SessionClosed.into());
    }
    let expires_at = chrono::DateTime::parse_from_rfc3339(&binding.expires_at)
        .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?
        .with_timezone(&chrono::Utc);
    anyhow::ensure!(expires_at > now, SessionMessageFailureV1::BindingExpired);
    Ok(())
}

/// Effect-point revalidation of the retained initiator: a revoked identity,
/// membership or capability, or a changed session owner, closes the emission.
fn revalidate_initiator(tx: &Connection, row: &MessageRow) -> anyhow::Result<()> {
    let account: crate::identity::HumanAccountV1 = identity::json_row(
        tx,
        "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
        &row.principal_key,
    )?
    .ok_or(failure(IdentityFailureV1::IdentityRevoked))?;
    anyhow::ensure!(account.active, failure(IdentityFailureV1::IdentityRevoked));
    let member: MembershipV1 = identity::json_row(
        tx,
        "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
        &row.membership_id,
    )?
    .ok_or(failure(IdentityFailureV1::ContextRevoked))?;
    anyhow::ensure!(
        member.active
            && member.principal.key()? == row.principal_key
            && member.organization_scope == row.organization_scope
            && member
                .capabilities
                .contains(&HumanCapabilityV1::SendSessionMessage),
        failure(IdentityFailureV1::ContextRevoked)
    );
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: row.session_id.clone(),
    };
    let grant: ResourceGrantV1 = identity::json_row(
        tx,
        "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
        &resource.key()?,
    )?
    .ok_or(failure(IdentityFailureV1::NotFound))?;
    anyhow::ensure!(
        grant.active && grant.owner.key()? == row.principal_key,
        failure(IdentityFailureV1::PermissionDenied)
    );
    Ok(())
}

fn revalidate_emission_binding(
    tx: &Connection,
    row: &MessageRow,
    expected_fingerprint: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    let binding =
        conversation_row(tx, &row.session_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
    require_emittable_binding(&binding, now)?;
    anyhow::ensure!(
        binding.status == SessionConversationStatusV1::Active,
        SessionMessageFailureV1::ChannelNotAdmitted
    );
    let fingerprint = binding.fingerprint()?;
    anyhow::ensure!(
        fingerprint == row.binding_fingerprint && fingerprint == expected_fingerprint,
        SessionMessageFailureV1::BindingChanged
    );
    revalidate_initiator(tx, row)
}

impl Store {
    /// Durably enqueue one short instruction on the caller's own owned source.
    /// The transaction revalidates Store authority, the personal
    /// SendSessionMessage context, the session's owner ACL, the retained
    /// binding (STOP, deadline, epoch pin) and the masking policy before the
    /// mandatory audit event and the message row commit together. The body is
    /// tenant business data: the audit retains metadata only. An exact replay
    /// returns the retained result to a still-authorized caller; divergent
    /// content under the same operation is a conflict.
    pub(crate) fn modern_session_message_send(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        scope: &AuthorizedScopeV1,
        request: &SessionMessageRequestV1,
        expected: HumanDiscoveryGeneration,
    ) -> anyhow::Result<(SessionMessageReceiptV1, Option<HumanDiscoveryGeneration>)> {
        request
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if human_modern::discovery_generation(&tx)? != expected {
            return Err(HumanDiscoveryStale.into());
        }
        // Authorization is revalidated before any replay disclosure.
        let current = authorize_send(&tx, scope, &self.tenant_id)?;
        let clock = workload::capture_control_time(&tx)?;
        let input_hash = request.input_hash()?;
        let content_hash = request.content_hash()?;
        let operation_id = request.operation_id.to_string();
        // An exact replay reconciles a lost response: it serves the retained
        // result to its still-authorized nominative initiator even if the
        // channel has since closed or advanced. STOP closes the right to emit
        // new input, not the reconciliation of an already retained record.
        if let Some(row) = message_row(&tx, &operation_id)? {
            anyhow::ensure!(row.input_hash == input_hash, IdentityFailureV1::Conflict);
            anyhow::ensure!(
                row.principal_key == current.principal().key()?,
                failure(IdentityFailureV1::PermissionDenied)
            );
            let event = stored_event(&tx, row.event_seq)?;
            anyhow::ensure!(event.hash == row.event_hash, IdentityFailureV1::Conflict);
            let mut receipt: SessionMessageReceiptV1 = serde_json::from_str(&row.receipt_json)?;
            receipt.replayed = true;
            return Ok((receipt, None));
        }
        // A new enqueue revalidates the retained binding (STOP, deadline, epoch
        // pin), the session's owner ACL and the masking policy before any effect.
        let session_id = request.session_id.to_string();
        let binding =
            conversation_row(&tx, &session_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
        anyhow::ensure!(
            binding.tenant_id == self.tenant_id,
            failure(IdentityFailureV1::NotFound)
        );
        require_emittable_binding(&binding, clock.at())?;
        anyhow::ensure!(
            binding.input_epoch == request.expected_input_epoch,
            SessionMessageFailureV1::BindingChanged
        );
        require_session_owner(&tx, &current, &session_id)?;
        // The approved masking policy must preserve the instruction exactly;
        // otherwise it is refused with a fixed reason before enqueue.
        anyhow::ensure!(
            self.redactor.text(&request.content) == request.content,
            SessionMessageFailureV1::ContentRequiresRedaction
        );
        // Bounded unresolved work per session and per principal.
        let unresolved_session: i64 = tx.query_row(
            "SELECT count(*) FROM session_messages WHERE session_id=?1 AND status!='acknowledged'",
            [&session_id],
            |r| r.get(0),
        )?;
        let principal_key = current.principal().key()?;
        let unresolved_principal: i64 = tx.query_row(
            "SELECT count(*) FROM session_messages WHERE principal_key=?1 AND status!='acknowledged'",
            [&principal_key],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            unresolved_session < MAX_UNRESOLVED_PER_SESSION
                && unresolved_principal < MAX_UNRESOLVED_PER_PRINCIPAL,
            SessionMessageFailureV1::CapacityExhausted
        );
        let binding_fingerprint = binding.fingerprint()?;
        let mut event = Event::new(ENQUEUED_KIND);
        event.id = message_event_id(b"ai-guard:session-message:v1", request.operation_id);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(session_id.clone());
        event.data = serde_json::to_value(Enqueued {
            version: 1,
            operation_id: operation_id.clone(),
            principal_key: principal_key.clone(),
            membership_id: current.membership_id().to_string(),
            organization_scope: current.organization_scope().to_string(),
            session_id: session_id.clone(),
            input_hash: input_hash.clone(),
            content_hash: content_hash.clone(),
            binding_fingerprint: binding_fingerprint.clone(),
            input_epoch: binding.input_epoch,
            content_bytes: request.content.len() as u64,
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session message enqueue requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        let receipt = SessionMessageReceiptV1 {
            version: 1,
            operation_id: request.operation_id,
            message_id: stored.event.id.clone(),
            session_id: request.session_id,
            input_hash: input_hash.clone(),
            content_hash: content_hash.clone(),
            binding_fingerprint: binding_fingerprint.clone(),
            input_epoch: binding.input_epoch,
            status: SessionMessageStatusV1::Queued,
            ack: None,
            audit_seq: stored.seq,
            audit_hash: stored.hash.clone(),
            replayed: false,
        };
        tx.execute(
            "INSERT INTO session_messages(operation_id,session_id,input_hash,content_hash,principal_key,membership_id,organization_scope,binding_fingerprint,input_epoch,content,status,event_seq,event_hash,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                operation_id,
                session_id,
                input_hash,
                content_hash,
                principal_key,
                current.membership_id(),
                current.organization_scope(),
                binding_fingerprint,
                binding.input_epoch as i64,
                request.content,
                status_str(SessionMessageStatusV1::Queued),
                stored.seq,
                stored.hash,
                serde_json::to_string(&receipt)?,
            ],
        )?;
        // The enqueue changes no identity authorization revision.
        self.commit_authorized(tx, &clock)?;
        Ok((receipt, None))
    }

    /// Read one retained message receipt (metadata only) for a still-authorized
    /// owner. Revocation of the identity, membership or context closes it.
    pub(crate) fn modern_session_message_receipt(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        operation_id: &str,
    ) -> anyhow::Result<SessionMessageReceiptV1> {
        let operation = crate::control::session_control::canonical_uuid(operation_id)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        self.with_modern_read(access, |tx| {
            let current = authorize_send(tx, scope, &self.tenant_id)?;
            let row = message_row(tx, &operation.to_string())?
                .ok_or(failure(IdentityFailureV1::NotFound))?;
            require_session_owner(tx, &current, &row.session_id)?;
            let event = stored_event(tx, row.event_seq)?;
            anyhow::ensure!(event.hash == row.event_hash, IdentityFailureV1::Conflict);
            let mut receipt: SessionMessageReceiptV1 = serde_json::from_str(&row.receipt_json)?;
            receipt.replayed = true;
            Ok(receipt)
        })
    }

    /// Read the current destination using the same authority as enqueue. The
    /// projection must match the latest durable launch, bind or close event.
    pub(crate) fn modern_session_conversation(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        session_id: &str,
    ) -> anyhow::Result<SessionConversationViewV1> {
        crate::control::session_control::canonical_uuid(session_id)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        self.with_modern_read(access, |tx| {
            let current = authorize_send(tx, scope, &self.tenant_id)?;
            require_session_owner(tx, &current, session_id)?;
            let (json, seq, hash): (String, i64, String) = tx
                .query_row(
                    "SELECT binding_json,event_seq,event_hash FROM session_conversations WHERE session_id=?1",
                    [session_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or(failure(IdentityFailureV1::NotFound))?;
            let event = tx.query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events \
                 WHERE kind IN (?1,?2,?3) AND seq>=?4 AND json_extract(payload,'$.session_id')=?5 \
                 ORDER BY seq DESC LIMIT 1",
                params![
                    super::session_prepare::LAUNCHED_KIND,
                    CONVERSATION_BOUND_KIND,
                    CONVERSATION_CLOSED_KIND,
                    seq,
                    session_id,
                ],
                row_event,
            )?;
            anyhow::ensure!(
                event.seq == seq && event.hash == hash,
                "session conversation projection event mismatch"
            );
            let key = if event.event.kind == super::session_prepare::LAUNCHED_KIND {
                super::session_prepare::validate_replay_event(tx, &event)?;
                "conversation"
            } else {
                validate_replay_event(tx, &event)?;
                "binding"
            };
            let binding: SessionConversationBindingV1 = serde_json::from_str(&json)?;
            let audited: SessionConversationBindingV1 =
                serde_json::from_value(event.event.data[key].clone())?;
            let namespace: String = tx.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                binding == audited
                    && binding.session_id == session_id
                    && binding.tenant_id == self.tenant_id
                    && binding.source_id == namespace,
                "session conversation projection payload mismatch"
            );
            Ok(SessionConversationViewV1 {
                version: binding.version,
                tenant_id: binding.tenant_id,
                source_id: binding.source_id,
                session_id: binding.session_id,
                input_epoch: binding.input_epoch,
                provider: binding.provider,
                protocol: binding.protocol,
                expires_at: binding.expires_at,
                status: binding.status,
            })
        })
    }

    /// Trusted-control read of the retained conversation binding (metadata
    /// only) for the broker composition. Never exposes the instruction body.
    pub(crate) fn session_conversation_binding_read(
        &self,
        access: &HumanSourceAccess<'_>,
        session_id: &str,
    ) -> anyhow::Result<SessionConversationBindingV1> {
        crate::control::session_control::canonical_uuid(session_id)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        self.with_modern_read(access, |tx| {
            if !super::session_prepare::installed(tx)? {
                return Err(failure(IdentityFailureV1::MigrationRequired));
            }
            let binding =
                conversation_row(tx, session_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
            anyhow::ensure!(
                binding.tenant_id == self.tenant_id,
                failure(IdentityFailureV1::NotFound)
            );
            Ok(binding)
        })
    }

    /// Trusted-control dispatch queue: the queued operation identities of one
    /// session in durable enqueue order. Metadata only — never the body. An
    /// issued, transmitted, acknowledged or unknown message is never listed
    /// again: delivery uncertainty is retained, never automatically resent.
    pub(crate) fn session_message_queue(
        &self,
        access: &HumanSourceAccess<'_>,
        session_id: &str,
    ) -> anyhow::Result<Vec<Uuid>> {
        crate::control::session_control::canonical_uuid(session_id)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        self.with_modern_read(access, |tx| {
            if !super::session_prepare::installed(tx)? {
                return Err(failure(IdentityFailureV1::MigrationRequired));
            }
            let binding =
                conversation_row(tx, session_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
            anyhow::ensure!(
                binding.tenant_id == self.tenant_id,
                failure(IdentityFailureV1::NotFound)
            );
            let mut statement = tx.prepare(
                "SELECT operation_id FROM session_messages WHERE session_id=?1 AND status='queued' ORDER BY event_seq",
            )?;
            let rows = statement.query_map([session_id], |r| r.get::<_, String>(0))?;
            let mut operations = Vec::new();
            for row in rows {
                operations.push(
                    Uuid::parse_str(&row?).map_err(|_| failure(IdentityFailureV1::Conflict))?,
                );
            }
            Ok(operations)
        })
    }

    /// Trusted-control emission issue: the durable latch before the first
    /// frame byte. Revalidates the exact retained binding at the effect point
    /// (STOP, deadline, resolved channel, fingerprint) and the retained
    /// initiator's current authority. After the issue, delivery is unknown
    /// until the complete write is observed; a replayed issue never re-emits.
    /// Called by the broker composition (`broker::conversation`).
    pub(crate) fn issue_session_message_emission_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        command: &IssueSessionMessageEmissionV1,
    ) -> anyhow::Result<SessionMessageEmissionV1> {
        command
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !super::session_prepare::installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let operation_id = command.operation_id.to_string();
        let row = message_row(&tx, &operation_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
        // A retained issue is never re-emitted, whatever the current status.
        let issue_id = message_event_id(
            b"ai-guard:session-message-emission:v1",
            command.operation_id,
        );
        anyhow::ensure!(
            event_by_id(&tx, &issue_id)?.is_none()
                && row.status == status_str(SessionMessageStatusV1::Queued),
            SessionMessageFailureV1::EmissionAlreadyIssued
        );
        let clock = workload::capture_control_time(&tx)?;
        revalidate_emission_binding(&tx, &row, &command.expected_binding_fingerprint, clock.at())?;
        let mut event = Event::new(EMISSION_ISSUED_KIND);
        event.id = issue_id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(row.session_id.clone());
        event.data = serde_json::to_value(EmissionIssued {
            version: 1,
            operation_id: operation_id.clone(),
            session_id: row.session_id.clone(),
            principal_key: row.principal_key.clone(),
            binding_fingerprint: row.binding_fingerprint.clone(),
            input_epoch: row.input_epoch as u64,
            content_hash: row.content_hash.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session message emission requires redaction"
        );
        self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        update_status(
            &tx,
            &operation_id,
            &row,
            SessionMessageStatusV1::Unknown,
            None,
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok(SessionMessageEmissionV1 {
            version: 1,
            operation_id: command.operation_id,
            session_id: row.session_id,
            input_epoch: row.input_epoch as u64,
            binding_fingerprint: row.binding_fingerprint,
            content_hash: row.content_hash,
            content: row.content,
        })
    }

    /// Validate the dispatch's live handoff before a single nonblocking write.
    /// This read never returns the body or issues another emission. The caller
    /// retains the same write epoch through the syscall to fence durable STOP.
    pub(crate) fn revalidate_session_message_emission_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        emission: &SessionMessageEmissionV1,
    ) -> anyhow::Result<()> {
        let access = HumanSourceAccess::write(epoch, source)?;
        self.with_modern_read(&access, |tx| {
            if !super::session_prepare::installed(tx)? {
                return Err(failure(IdentityFailureV1::MigrationRequired));
            }
            let row = message_row(tx, &emission.operation_id.to_string())?
                .ok_or(failure(IdentityFailureV1::NotFound))?;
            let clock = workload::capture_control_time(tx)?;
            revalidate_emission_binding(tx, &row, &emission.binding_fingerprint, clock.at())?;
            anyhow::ensure!(
                row.session_id == emission.session_id
                    && u64::try_from(row.input_epoch)? == emission.input_epoch
                    && row.content_hash == emission.content_hash,
                SessionMessageFailureV1::BindingChanged
            );
            let issue_id = message_event_id(
                b"ai-guard:session-message-emission:v1",
                emission.operation_id,
            );
            anyhow::ensure!(
                row.status == status_str(SessionMessageStatusV1::Unknown)
                    && event_by_id(tx, &issue_id)?
                        .is_some_and(|stored| stored.event.kind == EMISSION_ISSUED_KIND),
                IdentityFailureV1::Conflict
            );
            Ok(())
        })
    }

    /// Record the observed complete frame write on the exact owned channel.
    /// Only an issued emission (delivery unknown) can become transmitted.
    pub(crate) fn record_session_message_transmitted_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        operation_id: Uuid,
    ) -> anyhow::Result<SessionMessageReceiptV1> {
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !super::session_prepare::installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let row = message_row(&tx, &operation_id.to_string())?
            .ok_or(failure(IdentityFailureV1::NotFound))?;
        let event_id = message_event_id(b"ai-guard:session-message-transmitted:v1", operation_id);
        if event_by_id(&tx, &event_id)?.is_some() {
            let mut receipt: SessionMessageReceiptV1 = serde_json::from_str(&row.receipt_json)?;
            receipt.replayed = true;
            return Ok(receipt);
        }
        anyhow::ensure!(
            row.status == status_str(SessionMessageStatusV1::Unknown),
            IdentityFailureV1::Conflict
        );
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(TRANSMITTED_KIND);
        event.id = event_id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(row.session_id.clone());
        event.data = serde_json::to_value(Transmitted {
            version: 1,
            operation_id: operation_id.to_string(),
            session_id: row.session_id.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session message transmission requires redaction"
        );
        self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        let receipt = update_status(
            &tx,
            &operation_id.to_string(),
            &row,
            SessionMessageStatusV1::Transmitted,
            None,
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok(receipt)
    }

    /// Record the exact correlated official protocol acknowledgement. Only a
    /// transmitted message can become acknowledged; a divergent receipt under
    /// the same operation is a conflict. The acknowledgement proves the
    /// provider accepted the input, never that the instruction was executed.
    pub(crate) fn record_session_message_acknowledged_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        operation_id: Uuid,
        ack: &SessionMessageAckV1,
    ) -> anyhow::Result<SessionMessageReceiptV1> {
        anyhow::ensure!(
            !ack.turn_id.is_empty() && ack.turn_id.len() <= 128,
            failure(IdentityFailureV1::InvalidRequest)
        );
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !super::session_prepare::installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let row = message_row(&tx, &operation_id.to_string())?
            .ok_or(failure(IdentityFailureV1::NotFound))?;
        let event_id = message_event_id(b"ai-guard:session-message-ack:v1", operation_id);
        if let Some(stored) = event_by_id(&tx, &event_id)? {
            let record: Acknowledged = serde_json::from_value(stored.event.data.clone())?;
            anyhow::ensure!(record.ack == *ack, IdentityFailureV1::Conflict);
            let mut receipt: SessionMessageReceiptV1 = serde_json::from_str(&row.receipt_json)?;
            receipt.replayed = true;
            return Ok(receipt);
        }
        anyhow::ensure!(
            row.status == status_str(SessionMessageStatusV1::Transmitted),
            IdentityFailureV1::Conflict
        );
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(ACKNOWLEDGED_KIND);
        event.id = event_id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(row.session_id.clone());
        event.data = serde_json::to_value(Acknowledged {
            version: 1,
            operation_id: operation_id.to_string(),
            session_id: row.session_id.clone(),
            ack: ack.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session message acknowledgement requires redaction"
        );
        self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        let receipt = update_status(
            &tx,
            &operation_id.to_string(),
            &row,
            SessionMessageStatusV1::Acknowledged,
            Some(ack.clone()),
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok(receipt)
    }

    /// Trusted-control binding of the resolved conversation channel. Offline
    /// and epoch-checked: the expected epoch must equal the retained one and
    /// the binding advances it explicitly. A closed conversation never
    /// reopens. No HTTP route binds a channel.
    pub(crate) fn bind_session_conversation_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        command: &BindSessionConversationV1,
    ) -> anyhow::Result<SessionConversationBindingV1> {
        command
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !super::session_prepare::installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let input_hash = command.input_hash()?;
        let event_id = message_event_id(
            b"ai-guard:session-conversation-bind:v1",
            command.operation_id,
        );
        if let Some(stored) = event_by_id(&tx, &event_id)? {
            let record: ConversationChanged = serde_json::from_value(stored.event.data.clone())?;
            anyhow::ensure!(record.input_hash == input_hash, IdentityFailureV1::Conflict);
            return Ok(record.binding);
        }
        let mut binding = conversation_row(&tx, &command.session_id)?
            .ok_or(failure(IdentityFailureV1::NotFound))?;
        anyhow::ensure!(
            binding.status != SessionConversationStatusV1::Closed,
            SessionMessageFailureV1::SessionClosed
        );
        anyhow::ensure!(
            binding.input_epoch == command.expected_input_epoch,
            SessionMessageFailureV1::BindingChanged
        );
        binding.broker_id = Some(command.channel.broker_id.clone());
        binding.broker_incarnation = Some(command.channel.broker_incarnation.clone());
        binding.worker_id = Some(command.channel.worker_id.clone());
        binding.attempt_id = Some(command.channel.attempt_id.clone());
        binding.runtime_instance = Some(command.channel.runtime_instance.clone());
        binding.provider = Some(command.channel.provider.clone());
        binding.protocol = Some(command.channel.protocol.clone());
        binding.input_epoch = command
            .expected_input_epoch
            .checked_add(1)
            .ok_or(failure(IdentityFailureV1::InvalidRequest))?;
        binding.status = SessionConversationStatusV1::Active;
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(CONVERSATION_BOUND_KIND);
        event.id = event_id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(command.session_id.clone());
        event.data = serde_json::to_value(ConversationChanged {
            version: 1,
            operation_id: command.operation_id.to_string(),
            session_id: command.session_id.clone(),
            input_hash,
            binding: binding.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session conversation bind requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        tx.execute(
            "UPDATE session_conversations SET binding_json=?1,event_seq=?2,event_hash=?3 WHERE session_id=?4",
            params![
                serde_json::to_string(&binding)?,
                stored.seq,
                stored.hash,
                command.session_id,
            ],
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok(binding)
    }

    /// Trusted-control close of the conversation (STOP priority): the right to
    /// emit is closed durably before the worker stop follows the broker's
    /// guard. A message never reopens it.
    pub(crate) fn close_session_conversation_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        command: &CloseSessionConversationV1,
    ) -> anyhow::Result<SessionConversationBindingV1> {
        command
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !super::session_prepare::installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let input_hash = command.input_hash()?;
        let event_id = message_event_id(
            b"ai-guard:session-conversation-close:v1",
            command.operation_id,
        );
        if let Some(stored) = event_by_id(&tx, &event_id)? {
            let record: ConversationChanged = serde_json::from_value(stored.event.data.clone())?;
            anyhow::ensure!(record.input_hash == input_hash, IdentityFailureV1::Conflict);
            return Ok(record.binding);
        }
        let mut binding = conversation_row(&tx, &command.session_id)?
            .ok_or(failure(IdentityFailureV1::NotFound))?;
        anyhow::ensure!(
            binding.status != SessionConversationStatusV1::Closed,
            SessionMessageFailureV1::SessionClosed
        );
        binding.status = SessionConversationStatusV1::Closed;
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(CONVERSATION_CLOSED_KIND);
        event.id = event_id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(command.session_id.clone());
        event.data = serde_json::to_value(ConversationChanged {
            version: 1,
            operation_id: command.operation_id.to_string(),
            session_id: command.session_id.clone(),
            input_hash,
            binding: binding.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session conversation close requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        tx.execute(
            "UPDATE session_conversations SET binding_json=?1,event_seq=?2,event_hash=?3 WHERE session_id=?4",
            params![
                serde_json::to_string(&binding)?,
                stored.seq,
                stored.hash,
                command.session_id,
            ],
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok(binding)
    }
}

#[cfg(test)]
#[path = "session_message_composition_tests.rs"]
mod composition_tests;
