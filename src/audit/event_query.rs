use anyhow::Context;
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use std::{error::Error, fmt};

#[derive(Default, Debug, Clone, Deserialize)]
pub struct EventQuery {
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub limit: Option<usize>,
    pub q: Option<String>,
    pub agent_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub project_id: Option<String>,
    pub kind: Option<String>,
    pub decision: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub order: Option<String>,
    pub format: Option<String>,
}

impl EventQuery {
    pub(crate) fn validated_dates(&self) -> anyhow::Result<[Option<DateTime<FixedOffset>>; 2]> {
        let from = self
            .from
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context(InvalidEventFilter("invalid from date"))?;
        let to = self
            .to
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context(InvalidEventFilter("invalid to date"))?;
        if self.q.as_ref().is_some_and(|search| search.len() > 256) {
            return Err(InvalidEventFilter("search too long").into());
        }
        Ok([from, to])
    }

    pub(crate) fn page_limit(&self) -> usize {
        self.limit.unwrap_or(100).clamp(1, 1000)
    }
}

#[derive(Debug)]
pub(crate) struct InvalidEventFilter(&'static str);

impl fmt::Display for InvalidEventFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for InvalidEventFilter {}
