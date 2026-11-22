//! `GET /human/events/recent`: the newest authorized events first.
//! Legacy retains its sealed stream start; owned metadata is snapshot-only.
//!
//! Additive read. It reuses the context, authorization, redaction, byte
//! bounds and traversal budget of the other human pages, and the existing
//! `events` audit action: no new audit kind, schema or dependency.
use super::*;

/// Cursor binding of this traversal. It can never be confused with the
/// cursor of `/human/events`, `/human/export` or `/human/stream`.
#[derive(Serialize)]
struct RecentKey {
    route: &'static str,
    limit: usize,
    view: HumanReadViewV1,
    resource: Option<ResourceRefV1>,
}

#[derive(Serialize)]
pub(super) struct HumanRecentEventPageV1 {
    version: u32,
    context_id: String,
    authorization_version: String,
    view: HumanReadViewV1,
    order: &'static str,
    coverage: &'static str,
    items: Vec<HumanEventItemV1>,
    /// Older authorized events exist below this page.
    has_more: bool,
    next_cursor: Option<String>,
    /// Legacy first page only: accepted by `/human/stream` with exactly the same
    /// `limit`, `view` and resource; the stream starts strictly after the
    /// snapshot bound of this page. No audit position is exposed.
    live_cursor: Option<String>,
    /// The current owned metadata profile keeps content/detail/export/SSE
    /// closed. This descriptive marker grants no permission. Legacy omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_mode: Option<&'static str>,
    read_receipt: HumanReadReceiptV1,
}

pub(super) async fn recent(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanRecentEventPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        recent_sync(&proof, &state, &headers, &query)
    })
    .await
}

fn recent_sync(
    proof: &VerifiedHuman,
    state: &GatewayState,
    headers: &HeaderMap,
    query: &PageQuery,
) -> ApiResult<Json<HumanRecentEventPageV1>> {
    let (context, tenant, scope) = selected_context(state, proof, headers)?;
    // The legacy live position is bound to the stream query. Merely deriving
    // this query key creates no stream cursor or stream capability in owned.
    let stream_key = query.key(Traversal::Stream)?;
    let key = RecentKey {
        route: "human_events_recent_v1",
        limit: stream_key.limit,
        view: stream_key.view,
        resource: stream_key.resource.clone(),
    };
    let first = query.cursor.is_none();
    let mut cursor = context
        .start(&key, query.cursor.as_deref())
        .map_err(state_failure)?;
    // A sealed older-page cursor always carries its bound; a first page never does.
    if first != cursor.upper_event.is_none() {
        return Err(invalid_cursor());
    }
    let filter = HumanEventFilterV1 {
        view: key.view,
        resource: key.resource.clone(),
        event_id: None,
        export: false,
        stream: false,
    };
    let owned_profile = state.owned_human.is_some();
    let page = if let Some(owned) = &state.owned_human {
        let HumanProofSource::Local(source) = &proof.source else {
            return Err(super::super::human_sources::unavailable());
        };
        owned.recent(
            *source,
            tenant.store.sqlite_api()?,
            &scope,
            &filter,
            cursor.upper_event,
            key.limit,
            &proof.redactor,
        )?
    } else {
        tenant
            .store
            .sqlite_api()?
            .human_events_recent(
                &scope,
                &filter,
                cursor.upper_event,
                key.limit,
                &proof.redactor,
            )
            .map_err(store_failure)?
    };
    // The real read and its receipt committed, and OwnedHumanGateway::read
    // released its epoch. Accepted usage/I/O permits still belong to run_human.
    #[cfg(test)]
    if owned_profile {
        super::event_read_test_hook::pause(&state.human)?;
    }
    reauthenticate(state, proof)?;
    if owned_profile {
        selected_context(state, proof, headers)?;
    }
    let live = if first && !owned_profile {
        let mut live = context.start(&stream_key, None).map_err(state_failure)?;
        live.after_event = page.upper_event;
        live.upper_event = None;
        Some(live)
    } else {
        None
    };
    let placeholder = || Some("0".repeat(CURSOR_HEX_BYTES));
    let mut body = HumanRecentEventPageV1 {
        version: 1,
        context_id: context.id.clone(),
        authorization_version: context.authorization_version.clone(),
        view: key.view,
        order: "newest_first",
        coverage: "authorized_selection",
        items: page.items,
        has_more: page.has_more,
        // Fixed-size placeholders, never published: the complete envelope is
        // counted against the traversal budget before any cursor is sealed.
        next_cursor: page.has_more.then(placeholder).flatten(),
        live_cursor: live.as_ref().and_then(|_| placeholder()),
        stream_mode: owned_profile.then_some("snapshot_only"),
        read_receipt: page.receipt,
    };
    let size = serde_json::to_vec(&body)
        .map_err(|_| invalid_request())?
        .len();
    if size > HUMAN_MAX_PAGE_BYTES {
        return Err(invalid_request());
    }
    cursor = advance_budget(&context, cursor, size)?;
    context.ensure_valid().map_err(state_failure)?;
    if body.has_more {
        // The next older page reads strictly below the oldest item returned.
        cursor.upper_event = Some(page.after_event.saturating_sub(1));
        body.next_cursor = Some(context.seal(&key, &cursor).map_err(state_failure)?);
    }
    if let Some(live) = live {
        body.live_cursor = Some(context.seal(&stream_key, &live).map_err(state_failure)?);
    }
    if serde_json::to_vec(&body)
        .map_err(|_| invalid_request())?
        .len()
        > HUMAN_MAX_PAGE_BYTES
    {
        return Err(invalid_request());
    }
    Ok(Json(body))
}
