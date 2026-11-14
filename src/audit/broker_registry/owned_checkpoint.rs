//! Owned checkpoint persistence and self views for the proposed
//! `broker_supervision_owned_v2` profile. The owned guard admits exactly the
//! retained descriptor format (14 or 15) on a scoped transaction; the legacy
//! `checkpoint::require_schema12` lane (12|13, unscoped) is neither consulted
//! nor widened. A checkpoint stays a `DeclaredPartial` list of at most 100
//! declared heads within 32 KiB, with `has_more`: an empty list is not zero
//! activity, and nothing here refreshes heartbeat liveness, changes a worker,
//! quota, grant or authority, or proves a runtime inventory.
use super::*;
use crate::{
    audit::broker_registry::owned_supervision::{
        OwnedBrokerWindow, OwnedProof, OwnedRetainedSourceFacts, classify, operation_precheck,
        owned_check,
    },
    broker_registry::owned_supervision::{
        OwnedBrokerCheckpointRequestV2, OwnedBrokerCheckpointResultV2,
        OwnedBrokerCheckpointSelfViewV2, OwnedChecked, OwnedSupervisionErrorCodeV2 as Code,
    },
};

/// Most recent registry operations of one broker examined first by the human
/// projection.
const HUMAN_CHECKPOINT_SCAN: usize = 128;
/// Most recent checkpoint publications of the tenant examined, at most once
/// per human page, for a broker whose own recent operations hold none (for
/// example a broker that publishes heartbeats only).
const HUMAN_CHECKPOINT_TENANT_SCAN: usize = 4096;
const CHECKPOINT_KIND: &str = "control.broker_registry.checkpoint";

/// Latest checkpoint of one broker for the human projection, found by two
/// bounded index scans in the caller's checked transaction.
pub(in crate::audit) enum OwnedCheckpointLookup {
    /// Latest publication, with freshness independent of heartbeat liveness.
    Found(Box<BrokerCheckpointObservationV1>),
    /// A complete history holds no checkpoint of this broker: either its
    /// whole operation history, or every checkpoint publication of the
    /// tenant, was examined.
    Absent,
    /// Neither the broker's recent operations nor the tenant's recent
    /// checkpoint publications hold one, and older ones exist: never
    /// reported as an absence.
    Unknown,
}

/// Latest checkpoint operation of each broker among the tenant's most
/// recent checkpoint publications, read through the `events(kind,seq)` index
/// and the unique operation `event_seq`, at most once per human page.
pub(in crate::audit) struct OwnedCheckpointWindow {
    latest: BTreeMap<String, i64>,
    /// Every checkpoint publication of the tenant was examined, so a broker
    /// missing from `latest` has none.
    complete: bool,
}

fn checkpoint_window(db: &Connection) -> anyhow::Result<OwnedCheckpointWindow> {
    let window = i64::try_from(HUMAN_CHECKPOINT_TENANT_SCAN)?;
    let rows = {
        let mut statement = db.prepare("SELECT c.seq,o.broker_id FROM (SELECT seq FROM events WHERE kind=?1 ORDER BY seq DESC LIMIT ?2) c LEFT JOIN broker_registry_operations o ON o.event_seq=c.seq ORDER BY c.seq DESC")?;
        statement
            .query_map(params![CHECKPOINT_KIND, window + 1], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let complete = rows.len() <= HUMAN_CHECKPOINT_TENANT_SCAN;
    let mut latest = BTreeMap::new();
    for (seq, broker) in rows.into_iter().take(HUMAN_CHECKPOINT_TENANT_SCAN) {
        // An event of this kind without a registry operation is not a broker
        // publication; newest first, so the first seq of a broker is its last.
        if let Some(broker) = broker {
            latest.entry(broker).or_insert(seq);
        }
    }
    Ok(OwnedCheckpointWindow { latest, complete })
}

/// Bounded lookup: at most `HUMAN_CHECKPOINT_SCAN` operation rows of this
/// broker through its `(broker_id,event_seq)` index; when they hold no
/// publication and older ones exist, the tenant's checkpoint window (read
/// once into `tenant` for the whole page). One checkpoint record is then
/// decoded and its commitment rechecked.
pub(in crate::audit) fn owned_checkpoint_lookup(
    db: &Connection,
    broker: &BrokerV1,
    evaluated_at: &str,
    tenant: &mut Option<OwnedCheckpointWindow>,
) -> anyhow::Result<OwnedCheckpointLookup> {
    let window = i64::try_from(HUMAN_CHECKPOINT_SCAN)?;
    let rows = {
        let mut statement = db.prepare("SELECT o.event_seq,e.kind FROM broker_registry_operations o JOIN events e ON e.seq=o.event_seq WHERE o.broker_id=?1 ORDER BY o.event_seq DESC LIMIT ?2")?;
        statement
            .query_map(params![broker.broker_id, window + 1], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let found = rows
        .iter()
        .take(HUMAN_CHECKPOINT_SCAN)
        .find(|(_, kind)| kind == CHECKPOINT_KIND)
        .map(|(seq, _)| *seq);
    let seq = match found {
        Some(seq) => seq,
        None if rows.len() <= HUMAN_CHECKPOINT_SCAN => return Ok(OwnedCheckpointLookup::Absent),
        None => {
            if tenant.is_none() {
                *tenant = Some(checkpoint_window(db)?);
            }
            let latest = tenant
                .as_ref()
                .and_then(|published| published.latest.get(&broker.broker_id).copied());
            let complete = tenant.as_ref().is_some_and(|published| published.complete);
            match latest {
                Some(seq) => seq,
                None if complete => return Ok(OwnedCheckpointLookup::Absent),
                None => return Ok(OwnedCheckpointLookup::Unknown),
            }
        }
    };
    let (stored, json) = db.query_row(
        "SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload,o.result_json FROM broker_registry_operations o JOIN events e ON e.seq=o.event_seq WHERE o.event_seq=?1 AND o.broker_id=?2",
        params![seq, broker.broker_id],
        |row| Ok((row_event(row)?, row.get::<_, String>(5)?)),
    )?;
    let retained = checkpoint::retained(stored, &json)?;
    let observation = checkpoint::observation(retained, broker, evaluated_at)?;
    Ok(OwnedCheckpointLookup::Found(Box::new(observation)))
}

impl Store {
    /// Owned checkpoint publication. Inside one `Business` transaction: the
    /// engaged identity is rebuilt, the request subject and commitments are
    /// compared with it and the private registry `Change` hash is recomputed
    /// against the shared mirror before any replay or mutation. The shared
    /// registry body revalidates the proof and either returns the original
    /// receipt for an exact replay or appends the audit event and projection.
    pub(crate) fn owned_broker_checkpoint(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
        request: &OwnedBrokerCheckpointRequestV2,
    ) -> OwnedChecked<OwnedBrokerCheckpointResultV2> {
        request.validate()?;
        let change = Change::Checkpoint {
            identity: IdentityEvidence::capture(identity),
            checkpoint: request.checkpoint.clone(),
        };
        self.owned_change_input(&change)?;
        let proof = OwnedProof::Windowed(window, identity);
        self.owned_business(access, &proof, true, |tx, clock| {
            let current = self.owned_current(tx, clock, facts, identity)?;
            let commitments = request.verify_for(&current.identity)?;
            let private = change.input_hash().map_err(|_| Code::InvalidRequest)?;
            owned_check(
                private == commitments.registry_input_hash,
                Code::ProtocolMismatch,
            )?;
            operation_precheck(tx, &change, &private)?;
            let mutation = self
                .persist_broker_change_checked(tx, clock, change, Some(identity))
                .map_err(|error| classify(self, &error))?;
            // The input hash covers the checkpoint, so an exact replay carries
            // the same publication as the retained request.
            let result = BrokerCheckpointResultV1 {
                checkpoint: request.checkpoint.clone(),
                receipt: mutation.receipt,
                replayed: mutation.replayed,
            };
            let value = OwnedBrokerCheckpointResultV2::new(
                current.identity,
                commitments.operation_commitment,
                result,
            )
            .map_err(|_| Code::ResponseUnpublishable)?;
            self.owned_public(&value)?;
            Ok(value)
        })
    }

    /// Identity and the latest checkpoint observation of this broker, read in
    /// one checked transaction. A historical or stale publication keeps its
    /// freshness; absence is an explicit `None` from a complete durable read.
    pub(crate) fn owned_broker_checkpoint_self(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedBrokerCheckpointSelfViewV2> {
        let proof = OwnedProof::Windowed(window, identity);
        self.owned_business(access, &proof, false, |tx, clock| {
            let fail = |error: anyhow::Error| classify(self, &error);
            let current = self.owned_current(tx, clock, facts, identity)?;
            let checkpoint = checkpoint::latest(tx, &current.broker.broker_id)
                .map_err(fail)?
                .map(|retained| {
                    checkpoint::observation(retained, &current.broker, &current.evaluated_at)
                })
                .transpose()
                .map_err(fail)?;
            let view = BrokerCheckpointViewV1 {
                version: 1,
                broker: current
                    .broker
                    .view_at(&current.evaluated_at)
                    .map_err(fail)?,
                checkpoint,
            };
            let value = OwnedBrokerCheckpointSelfViewV2::new(current.identity, view)
                .map_err(|_| Code::ResponseUnpublishable)?;
            self.owned_public(&value)?;
            Ok(value)
        })
    }
}
