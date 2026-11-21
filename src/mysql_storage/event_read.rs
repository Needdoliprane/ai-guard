use crate::{audit::EventQuery, model::StoredEvent};
use anyhow::Result;
use chrono::{DateTime, Datelike, FixedOffset};
use std::collections::VecDeque;

/// Collect a bounded page during the prefix scan. Nothing is returned until
/// the store has also verified the head and every projection.
pub(super) struct Selection<'q> {
    query: &'q EventQuery,
    received_from: Option<i64>,
    received_through: Option<i64>,
    unsatisfiable_time: bool,
    search: Option<String>,
    ascending: bool,
    limit: usize,
    events: VecDeque<StoredEvent>,
}

impl<'q> Selection<'q> {
    pub(super) fn new(query: &'q EventQuery) -> Result<Self> {
        let [from, to] = query.validated_dates()?;
        let received_from = query
            .from
            .as_deref()
            .zip(from)
            .map(|(raw, time)| query_millis(raw, time));
        let received_through = query
            .to
            .as_deref()
            .zip(to)
            .map(|(raw, time)| query_millis(raw, time));
        Ok(Self {
            query,
            received_from: received_from.flatten(),
            received_through: received_through.flatten(),
            unsatisfiable_time: received_from == Some(None) || received_through == Some(None),
            search: query.q.as_ref().map(|search| search.to_ascii_lowercase()),
            ascending: query.order.as_deref() == Some("asc"),
            limit: query.page_limit(),
            events: VecDeque::new(),
        })
    }

    pub(super) fn inspect(&mut self, stored: &StoredEvent, payload: &str) -> Result<()> {
        if self.unsatisfiable_time || (self.ascending && self.events.len() == self.limit) {
            return Ok(());
        }
        let q = self.query;
        let e = &stored.event;
        if q.before.is_some_and(|seq| stored.seq >= seq)
            || q.after.is_some_and(|seq| stored.seq <= seq)
            || q.agent_id
                .as_ref()
                .is_some_and(|id| e.agent_id.as_ref() != Some(id))
            || q.sandbox_id
                .as_ref()
                .is_some_and(|id| e.sandbox_id.as_ref() != Some(id))
            || q.project_id
                .as_ref()
                .is_some_and(|id| e.project_id.as_ref() != Some(id))
            || q.kind.as_ref().is_some_and(|kind| &e.kind != kind)
            || q.decision.as_deref().is_some_and(|outcome| {
                e.decision.as_ref().map(|d| d.outcome.as_str()) != Some(outcome)
            })
        {
            return Ok(());
        }
        if self.received_from.is_some() || self.received_through.is_some() {
            let received = DateTime::parse_from_rfc3339(&stored.received_at)?.timestamp_millis();
            if self.received_from.is_some_and(|from| received < from)
                || self
                    .received_through
                    .is_some_and(|through| received > through)
            {
                return Ok(());
            }
        }
        if self
            .search
            .as_ref()
            .is_some_and(|search| !payload.to_ascii_lowercase().contains(search))
        {
            return Ok(());
        }
        if self.events.len() == self.limit {
            self.events.pop_front();
        }
        self.events.push_back(stored.clone());
        Ok(())
    }

    pub(super) fn into_events(self) -> Vec<StoredEvent> {
        if self.ascending {
            self.events.into()
        } else {
            self.events.into_iter().rev().collect()
        }
    }
}

fn query_millis(raw: &str, time: DateTime<FixedOffset>) -> Option<i64> {
    // These RFC3339 values have no value in the existing SQLite julianday
    // filter, so their comparison selects no rows.
    if raw.as_bytes().get(10) == Some(&b't')
        || time.timestamp_subsec_nanos() >= 1_000_000_000
        || time.offset().local_minus_utc().unsigned_abs() >= 15 * 3600
        || time.naive_utc().year() > 9999
    {
        return None;
    }
    // Round to milliseconds with the same per-second clamp as julianday.
    let ms = ((time.timestamp_subsec_nanos() + 500_000) / 1_000_000).min(999);
    Some(time.timestamp() * 1000 + i64::from(ms))
}
