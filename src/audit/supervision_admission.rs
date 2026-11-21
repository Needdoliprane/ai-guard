//! Shared record codec for a supervised admission. Storage owns authority and projections.
pub(crate) use super::control::supervision::{Input, Record};
use super::worker_admission::{self, ControlRecord};
use crate::{
    control::{
        AdmissionResultV1, ControlFailure, MutationReceiptV1, QuotaPolicyV1, SupervisedAdmissionV1,
        conflict, invalid,
    },
    model::{Event, StoredEvent},
    redaction::Redactor,
};
use anyhow::Result;

pub(crate) const ACTION: &str = "supervised_admit";
pub(crate) const KIND: &str = "control.supervision.transition";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub(crate) worker_id: String,
    pub(crate) intent_hash: String,
    pub(crate) input: Input,
}

pub(crate) fn admission_input(
    request: &SupervisedAdmissionV1,
    quota: &QuotaPolicyV1,
) -> Result<Input> {
    request.validate()?;
    quota.validate()?;
    Ok(Input::Admission(Box::new(request.clone())))
}

pub(crate) fn validate_input(input: &Input, redactor: &Redactor) -> Result<String> {
    let input_hash = input.hash()?;
    let value = serde_json::to_value(input)?;
    invalid(redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536)?;
    Ok(input_hash)
}

/// Verify an exact replay after storage looks up the operation in its checked transaction.
pub(crate) fn require_replay(record: &Record, input: &Input, input_hash: &str) -> Result<()> {
    conflict(record.input.hash()? == input_hash && record.input.action() == input.action())
}

impl Record {
    pub(crate) fn new(input: Input, worker_record: Option<ControlRecord>) -> Self {
        Self {
            version: 2,
            input,
            worker_record,
        }
    }

    /// Check the inner worker mutation against the outer supervised request.
    /// Decision, quota, worker projection and intent uniqueness are checked in storage.
    pub(crate) fn admission(&self) -> Result<(&SupervisedAdmissionV1, &ControlRecord)> {
        let Input::Admission(request) = &self.input else {
            return Err(ControlFailure::InvalidRequest.into());
        };
        let legacy = self
            .worker_record
            .as_ref()
            .ok_or(ControlFailure::InvalidRequest)?;
        invalid(
            legacy.action == "admit"
                && legacy.input_hash == request.admission.input_hash()?
                && legacy.operation_id == request.admission.operation_id,
        )?;
        if let Some(worker) = &legacy.worker {
            invalid(
                serde_json::to_value(&worker.launch)?
                    == serde_json::to_value(&request.admission.launch)?
                    && serde_json::to_value(&worker.receipt)?
                        == serde_json::to_value(&request.admission.receipt)?,
            )?;
        }
        Ok((request, legacy))
    }
}

pub(crate) fn event(record: &Record, session_id: Option<&str>) -> Result<Event> {
    let mut event = Event::new(record.input.kind());
    event.data = serde_json::to_value(record)?;
    event.session_id = session_id.map(str::to_owned);
    Ok(event)
}

pub(crate) fn require_prepared(
    prepared: &Event,
    record: &Record,
    redactor: &Redactor,
) -> Result<()> {
    let mut expected = serde_json::to_value(record)?;
    if let Some(policy) = expected.pointer_mut("/worker_record/admission_policy") {
        *policy = redactor.value(policy);
    }
    invalid(prepared.data == expected)
}

/// Decode and verify the existing envelope. The caller checks the supervision boundary.
pub(crate) fn decode(stored: &StoredEvent) -> Result<Record> {
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    invalid(
        record.version == 2
            && stored.event.kind == record.input.kind()
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed",
    )?;
    Ok(record)
}

pub(crate) fn receipt_for(
    stored: &StoredEvent,
    record: &Record,
) -> anyhow::Result<MutationReceiptV1> {
    Ok(MutationReceiptV1 {
        operation_id: record.input.operation_id().into(),
        worker_id: record.input.worker_id(record).map(str::to_owned),
        revision: record
            .worker_record
            .as_ref()
            .and_then(|r| r.worker.as_ref())
            .map(|w| w.revision),
        input_hash: record.input.hash()?,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    })
}

pub(crate) fn result(
    stored: &StoredEvent,
    record: &Record,
    replayed: bool,
) -> Result<AdmissionResultV1> {
    invalid(matches!(&record.input, Input::Admission(_)))?;
    let legacy = record
        .worker_record
        .as_ref()
        .ok_or(ControlFailure::InvalidRequest)?;
    let mut result = worker_admission::admission_result(stored, legacy);
    result.receipt = receipt_for(stored, record)?;
    result.replayed = replayed;
    Ok(result)
}
