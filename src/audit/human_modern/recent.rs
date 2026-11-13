//! Own-source recent metadata, through the existing zero-wait checked read.
use super::*;

impl Store {
    pub(crate) fn modern_human_events_recent(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanEventItemV1>> {
        Self::validate_human_events_recent(filter, upper, limit)?;
        anyhow::ensure!(
            filter.view == HumanReadViewV1::Metadata,
            "owned recent supports metadata only"
        );
        self.with_modern_read(access, |tx| {
            self.human_events_recent_checked(tx, scope, filter, upper, limit, transient)
        })
    }
}

#[cfg(test)]
mod tests;
