use crate::{
    audit::{
        decision_codec::{self, DecisionFailure},
        event_codec::require_ordinary_kind,
    },
    model::{DecisionRequest, Event, StoredEvent, event_kind_in_namespace},
};
use anyhow::{Result, ensure};
use mysql_async::{Row, Transaction, prelude::Queryable};
use serde_json::Value;
use uuid::Uuid;

use super::{audit::decode_event, driver_error};

pub(super) fn require_ordinary(kind: &str) -> Result<()> {
    require_ordinary_kind(kind)?;
    ensure!(
        !event_kind_in_namespace(kind, decision_codec::KIND),
        "decision events require typed mutation"
    );
    Ok(())
}

#[derive(PartialEq, Eq)]
pub(super) struct DecisionKey<'a> {
    request_id: &'a str,
    human_approved: u8,
    request_hash: &'a str,
}

impl<'a> DecisionKey<'a> {
    pub(super) fn from_request(request: &'a DecisionRequest, request_hash: &'a str) -> Self {
        Self {
            request_id: &request.request_id,
            human_approved: u8::from(request.human_approved),
            request_hash,
        }
    }

    pub(super) fn from_event(event: &'a Event) -> Result<Self> {
        let ambiguous = DecisionFailure::AmbiguousHistory;
        let request_id = event
            .data
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or(ambiguous)?;
        let request_hash = event
            .data
            .get("request_hash")
            .and_then(Value::as_str)
            .ok_or(ambiguous)?;
        let approved = event
            .data
            .get("human_approved")
            .and_then(Value::as_bool)
            .ok_or(ambiguous)?;
        ensure!(
            event.kind == decision_codec::KIND
                && event.source == "gateway"
                && event.provenance == "observed"
                && event.decision.is_some()
                && event.data.get("profile").and_then(Value::as_str).is_some()
                && event.data.get("jev").is_some()
                && Uuid::parse_str(request_id).is_ok()
                && request_hash.len() == 64
                && request_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            ambiguous
        );
        Ok(Self {
            request_id,
            human_approved: u8::from(approved),
            request_hash,
        })
    }

    pub(super) async fn persist(&self, tx: &mut Transaction<'_>, seq: i64) -> Result<()> {
        tx.exec_drop(
            "INSERT INTO ai_guard_audit_decision (request_id,human_approved,request_hash,event_seq) VALUES (?,?,?,?)",
            (self.request_id, self.human_approved, self.request_hash, seq),
        )
        .await
        .map_err(driver_error)?;
        Ok(())
    }
}

pub(super) fn verify_projection(
    event: &Event,
    request_id: Option<&str>,
    approved: Option<u8>,
    request_hash: Option<&str>,
) -> Result<bool> {
    match (request_id, approved, request_hash) {
        (Some(request_id), Some(human_approved), Some(request_hash)) => {
            let expected = DecisionKey {
                request_id,
                human_approved,
                request_hash,
            };
            ensure!(
                DecisionKey::from_event(event)? == expected,
                DecisionFailure::AmbiguousHistory
            );
            Ok(true)
        }
        (None, None, None) => {
            ensure!(
                event.kind != decision_codec::KIND,
                "MySQL decision event is missing its index receipt"
            );
            require_ordinary(&event.kind)?;
            Ok(false)
        }
        _ => Err(DecisionFailure::AmbiguousHistory.into()),
    }
}

pub(super) async fn lookup(
    tx: &mut Transaction<'_>,
    request: &DecisionRequest,
    hash: &str,
) -> Result<Option<StoredEvent>> {
    let rows = tx
        .exec::<Row, _, _>(
            "SELECT e.seq,e.event_id,e.received_at,e.prev_hash,e.hash,e.payload \
             FROM ai_guard_audit_decision d JOIN ai_guard_audit_event e ON e.seq=d.event_seq \
             WHERE d.request_id=? ORDER BY e.seq LIMIT 3",
            (&request.request_id,),
        )
        .await
        .map_err(driver_error)?;
    let events = rows
        .into_iter()
        .map(|row| decode_event(row).map(|entry| entry.stored))
        .collect::<Result<Vec<_>>>()?;
    decision_codec::find_receipt(events, request, hash)
}
