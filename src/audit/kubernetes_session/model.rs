use super::*;
use control_recovery::JournalAuthorization;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdentityEvidence {
    pub tenant_id: String,
    pub broker_id: String,
    pub incarnation_id: String,
    pub credential_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
}
impl IdentityEvidence {
    pub(super) fn capture(identity: &BrokerIdentityV1, broker: &BrokerV1) -> Self {
        Self {
            tenant_id: identity.tenant_id().into(),
            broker_id: identity.broker_id().into(),
            incarnation_id: broker.incarnation_id.clone(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        }
    }
    pub(super) fn identity(&self) -> anyhow::Result<BrokerIdentityV1> {
        uuid(&self.broker_id)?;
        uuid(&self.incarnation_id)?;
        uuid(&self.credential_id)?;
        require(
            self.binding_revision > 0
                && self.grant_revision > 0
                && i64::try_from(self.binding_revision).is_ok()
                && i64::try_from(self.grant_revision).is_ok(),
        )?;
        BrokerIdentityV1::authenticated(
            self.tenant_id.clone(),
            self.broker_id.clone(),
            self.credential_id.clone(),
            self.binding_revision,
            self.grant_revision,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Creator {
    ThisReserve,
    Reservation {
        reservation_seq: i64,
        reservation_hash: String,
        actor: IdentityEvidence,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Actor {
    Broker {
        tenant_id: String,
        broker_id: String,
        incarnation_id: String,
        credential_id: String,
        binding_revision: u64,
        grant_revision: u64,
    },
    ControlSharedRunner {
        tenant_id: String,
        mechanism: String,
        role: String,
        auth_origin: String,
        auth_epoch_id: String,
        request_auth_id: String,
        authenticated_at: String,
    },
}
impl Actor {
    pub(super) fn broker(identity: &IdentityEvidence) -> Self {
        Self::Broker {
            tenant_id: identity.tenant_id.clone(),
            broker_id: identity.broker_id.clone(),
            incarnation_id: identity.incarnation_id.clone(),
            credential_id: identity.credential_id.clone(),
            binding_revision: identity.binding_revision,
            grant_revision: identity.grant_revision,
        }
    }
    pub(super) fn identity(&self) -> anyhow::Result<IdentityEvidence> {
        match self {
            Self::Broker {
                tenant_id,
                broker_id,
                incarnation_id,
                credential_id,
                binding_revision,
                grant_revision,
            } => Ok(IdentityEvidence {
                tenant_id: tenant_id.clone(),
                broker_id: broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
                credential_id: credential_id.clone(),
                binding_revision: *binding_revision,
                grant_revision: *grant_revision,
            }),
            Self::ControlSharedRunner { .. } => Err(Failure::PermissionDenied.into()),
        }
    }
    pub(super) fn tenant_id(&self) -> &str {
        match self {
            Self::Broker { tenant_id, .. } | Self::ControlSharedRunner { tenant_id, .. } => {
                tenant_id
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Change {
    Reserve,
    CloseBeforeEffect {
        close: KubernetesSessionReservationCloseV2,
    },
}
impl Change {
    pub(super) fn operation_id<'a>(
        &'a self,
        intent: &'a KubernetesSessionReservationIntentV2,
    ) -> &'a str {
        match self {
            Self::Reserve => intent.reservation_id(),
            Self::CloseBeforeEffect { close } => &close.operation_id,
        }
    }
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Reserve => "control.kubernetes.session.reserved",
            Self::CloseBeforeEffect { .. } => "control.kubernetes.session.closed_before_effect",
        }
    }
    pub(super) fn input_hash(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        tenant: &str,
    ) -> anyhow::Result<String> {
        match self {
            Self::Reserve => intent.input_hash(tenant),
            Self::CloseBeforeEffect { close } => close.input_hash(),
        }
    }
}

#[derive(Clone, Serialize)]
pub(super) struct Record {
    pub version: u32,
    pub input_hash: String,
    pub received_at: String,
    pub actor: Actor,
    pub creator: Creator,
    #[serde(rename = "control_decision")]
    pub authorization: JournalAuthorization,
    pub intent: KubernetesSessionReservationIntentV2,
    pub previous_revision: u64,
    pub change: Change,
    pub post_state: KubernetesSessionReservationStateDataV2,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordWire {
    version: u32,
    input_hash: String,
    received_at: String,
    actor: Actor,
    creator: Creator,
    #[serde(rename = "control_decision")]
    authorization: JournalAuthorization,
    intent: Value,
    previous_revision: u64,
    change: Change,
    post_state: KubernetesSessionReservationStateDataV2,
}

pub(super) fn transition(
    intent: &KubernetesSessionReservationIntentV2,
    previous: Option<&KubernetesSessionReservationStateDataV2>,
    change: &Change,
    tenant: &str,
) -> anyhow::Result<KubernetesSessionReservationStateDataV2> {
    intent.validate()?;
    match change {
        Change::Reserve => {
            conflict(previous.is_none())?;
            Ok(KubernetesSessionReservationStateDataV2 {
                state: KubernetesSessionReservationStateV2::Reserved,
                revision: 1,
                quota_held: true,
                effect_fence: KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 },
                last_close: None,
            })
        }
        Change::CloseBeforeEffect { close } => {
            close.validate()?;
            let old = previous.ok_or(Failure::NotFound)?;
            old.validate()?;
            conflict(
                close.reservation_id == intent.reservation_id()
                    && close.reservation_input_hash == intent.input_hash(tenant)?
                    && old.state == KubernetesSessionReservationStateV2::Reserved
                    && old.quota_held
                    && close.expected_revision == old.revision,
            )?;
            Ok(KubernetesSessionReservationStateDataV2 {
                state: KubernetesSessionReservationStateV2::ClosedBeforeEffect,
                revision: 2,
                quota_held: false,
                effect_fence: old.effect_fence.clone(),
                last_close: Some(close.clone()),
            })
        }
    }
}
pub(super) fn snapshot(
    intent: &KubernetesSessionReservationIntentV2,
    state: KubernetesSessionReservationStateDataV2,
    tenant: &str,
) -> anyhow::Result<KubernetesSessionReservationSnapshotV2> {
    let snapshot = KubernetesSessionReservationSnapshotV2 {
        version: 2,
        intent: intent.clone(),
        input_hash: intent.input_hash(tenant)?,
        session_hash: intent.session_hash(tenant)?,
        attempt_hash: intent.attempt_hash(tenant)?,
        profile_hash: intent.profile_hash()?,
        state,
    };
    snapshot.validate(tenant)?;
    Ok(snapshot)
}
pub(super) fn record(event: &Event) -> anyhow::Result<Record> {
    require(serde_json::to_vec(event)?.len() <= 128 * 1024)?;
    let wire: RecordWire =
        serde_json::from_value(event.data.clone()).map_err(|_| Failure::InvalidRequest)?;
    let record = Record {
        version: wire.version,
        input_hash: wire.input_hash,
        received_at: wire.received_at,
        actor: wire.actor,
        creator: wire.creator,
        authorization: wire.authorization,
        intent: KubernetesSessionReservationIntentV2::retained_from_value(wire.intent)?,
        previous_revision: wire.previous_revision,
        change: wire.change,
        post_state: wire.post_state,
    };
    let ids = record.intent.ids()?;
    let operation = record.change.operation_id(&record.intent);
    conflict(
        record.version == 2
            && event.id == operation
            && event.kind == record.change.kind()
            && record.input_hash
                == record
                    .change
                    .input_hash(&record.intent, record.actor.tenant_id())?
            && event.timestamp == record.received_at
            && format_time(parse_time(&record.received_at)?) == record.received_at
            && record.received_at.len() == 24
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "observed"
            && event.session_id.as_deref() == Some(ids.session_id.to_string().as_str())
            && event.correlation_id.as_deref() == Some(record.intent.reservation_id())
            && event.project_id.is_none()
            && event.sandbox_id.is_none()
            && event.agent_id.is_none()
            && event.parent_agent_id.is_none()
            && event.trace_id.is_none()
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none(),
    )?;
    record.post_state.validate()?;
    if record.authorization.control().is_some() {
        control_recovery::validate_record(&record)?;
        return Ok(record);
    }
    let actor = record.actor.identity()?;
    actor.identity()?;
    let authorization = record.authorization.broker()?;
    authorization.validate(&record.intent, &actor, parse_time(&record.received_at)?)?;
    authorization.allows(&record.change, &record.intent)?;
    permission(actor == authorization.actor)?;
    match (&record.change, &record.creator) {
        (Change::Reserve, Creator::ThisReserve) => conflict(record.previous_revision == 0)?,
        (
            Change::CloseBeforeEffect { .. },
            Creator::Reservation {
                reservation_seq,
                reservation_hash,
                actor: creator,
            },
        ) => {
            creator.identity()?;
            hash(reservation_hash)?;
            conflict(
                *reservation_seq > 0
                    && record.previous_revision == 1
                    && creator.broker_id == actor.broker_id
                    && creator.tenant_id == actor.tenant_id
                    && creator.incarnation_id == ids.incarnation_id.to_string(),
            )?;
        }
        _ => return Err(Failure::Conflict.into()),
    }
    Ok(record)
}
pub(super) fn event(record: &Record) -> anyhow::Result<Event> {
    let mut event = Event::new(record.change.kind());
    event.id = record.change.operation_id(&record.intent).into();
    event.timestamp = record.received_at.clone();
    event.session_id = Some(record.intent.ids()?.session_id.to_string());
    event.correlation_id = Some(record.intent.reservation_id().into());
    event.data = serde_json::to_value(record)?;
    // The same typed parser governs fresh appends and capacity/reopen scans.
    self::record(&event)?;
    Ok(event)
}
pub(super) fn receipt(
    stored: &StoredEvent,
    record: &Record,
) -> anyhow::Result<KubernetesSessionReservationReceiptV2> {
    Ok(KubernetesSessionReservationReceiptV2 {
        version: 2,
        operation: match record.change {
            Change::Reserve => KubernetesSessionReservationOperationV2::Reserve,
            Change::CloseBeforeEffect { .. } => {
                KubernetesSessionReservationOperationV2::CloseBeforeEffect
            }
        },
        operation_id: record.change.operation_id(&record.intent).into(),
        input_hash: record.input_hash.clone(),
        control_namespace_uuid: record.intent.ids()?.control_namespace_uuid.to_string(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    })
}
