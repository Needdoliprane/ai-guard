//! Recent metadata borrows only the authenticated own source and its epoch.
use super::*;
use crate::audit::{HumanEventFilterV1, HumanStorePageV1};

impl OwnedHumanGateway {
    pub(in crate::api) fn recent(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        upper: Option<i64>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<HumanStorePageV1<HumanEventItemV1>> {
        if filter.view != HumanReadViewV1::Metadata
            || filter.event_id.is_some()
            || filter.export
            || filter.stream
        {
            return Err(unavailable());
        }
        self.read(index, store, |access| {
            store.modern_human_events_recent(access, scope, filter, upper, limit, redactor)
        })
    }
}
