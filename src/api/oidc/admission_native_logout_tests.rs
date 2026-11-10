//! BEGIN/FRefresh/END ports must be supplied by the approved Core composition.
//! Receipt omission models an ACK boundary; no physical crash is claimed.
use super::*;

#[path = "admission_qualification_followup_tests.rs"]
mod qualification_followup;

#[tokio::test]
async fn known_session_id_without_its_bearer_cannot_prepare_logout_or_reserve_l() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    pending(&fixture).await;
    let records = oidc.audit.verify().unwrap().records;
    let consumed = start_count(oidc);
    let response = gateway_router(fixture.state.clone()).oneshot(Request::post("/auth/oidc/logout")
        .header("content-type", "application/json").header("authorization", "Bearer synthetic-unverified-logout-token")
        .body(Body::from(serde_json::to_vec(&json!({"version":1,
            "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation})).unwrap())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 35_000)
        .await
        .unwrap();
    let reply: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply["local_session"], "unknown");
    {
        let registry = oidc.registry.lock().unwrap();
        let pending = registry.sessions[&fixture.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        assert!(pending.native_logout.is_none() && pending.native_close.is_none());
        assert_eq!(native_owners(&registry).count(), 1);
    }
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    assert_eq!(start_count(oidc), consumed);
    assert_eq!(logout_request(&fixture).await.0, StatusCode::OK);
    assert_eq!(oidc.audit.verify().unwrap().records, records + 3);
}

#[tokio::test]
async fn a_refresh_final_before_first_logout_begin_stays_closed_without_a_fabricated_promise() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let original = begin_refresh(
        oidc,
        &fixture.raw,
        fixture.binding.session_id,
        fixture.binding.generation,
        SessionKind::NativeCli,
        None,
    )
    .unwrap();
    let (refresh, choice) = retain_native_close_choice(
        &mut oidc.registry.lock().unwrap(),
        &fixture.binding,
        original.record.operation_id,
        &original.receipt,
        Outcome::Rejected,
        Reason::InvalidResponse,
    )
    .unwrap();
    // Actual durable FRefresh, deliberately not acknowledged into its keeper;
    // no ILogout existed at its commit. BEGIN cannot invent an earlier fact.
    let final_record = result(
        refresh,
        original.receipt.clone(),
        choice.outcome,
        choice.reason,
        Some(fixture.binding.principal.clone()),
    );
    let final_ack = oidc
        .audit
        .close_session(
            final_record.clone(),
            fixture.binding.generation,
            fixture.binding.access_fingerprint,
        )
        .unwrap();
    let records = oidc.audit.verify().unwrap().records;
    for _ in 0..2 {
        let (status, reply) = logout_request(&fixture).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reply,
            json!({"version":1,"local_session":"unknown",
            "provider_logout":"not_attempted","audit":"unavailable"})
        );
    }
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert_eq!(
        oidc.audit
            .close_session(
                final_record,
                fixture.binding.generation,
                fixture.binding.access_fingerprint
            )
            .unwrap(),
        final_ack
    );
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    assert!(oidc.authenticate(&fixture.raw).is_err());
    drop(original);
    assert_eq!(oidc.audit.verify().unwrap().records, records);
}

struct Prefix {
    refresh: AuthenticationRecordV1,
    refresh_intention: AuthenticationReceiptV1,
    logout: AuthenticationRecordV1,
    choice: NativeCloseChoice,
}

fn prepare(fixture: &Fixture) -> Prefix {
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut registry = oidc.registry.lock().unwrap();
    prepare_native_logout(oidc, &mut registry, &fixture.binding).unwrap();
    let pending = registry.sessions[&fixture.binding.session_id.to_string()]
        .pending_refresh
        .as_ref()
        .unwrap();
    Prefix {
        refresh: pending.record.clone(),
        refresh_intention: pending.receipt.clone(),
        logout: pending.native_logout.as_ref().unwrap().intention.clone(),
        choice: pending.native_close.unwrap(),
    }
}
fn begin_ledger(fixture: &Fixture, prefix: &Prefix) -> AuthenticationReceiptV1 {
    fixture
        .state
        .oidc
        .as_ref()
        .unwrap()
        .audit
        .begin_native_logout_during_refresh(
            prefix.logout.clone(),
            fixture.binding.clone(),
            prefix.refresh.operation_id,
            prefix.refresh_intention.clone(),
        )
        .unwrap()
}
fn close_ledger(fixture: &Fixture, prefix: &Prefix) -> AuthenticationReceiptV1 {
    fixture
        .state
        .oidc
        .as_ref()
        .unwrap()
        .audit
        .close_session(
            result(
                prefix.refresh.clone(),
                prefix.refresh_intention.clone(),
                prefix.choice.outcome,
                prefix.choice.reason,
                Some(fixture.binding.principal.clone()),
            ),
            fixture.binding.generation,
            fixture.binding.access_fingerprint,
        )
        .unwrap()
}
fn end_ledger(
    fixture: &Fixture,
    prefix: &Prefix,
    begin: &AuthenticationReceiptV1,
    refresh_result: &AuthenticationReceiptV1,
) -> AuthenticationReceiptV1 {
    fixture
        .state
        .oidc
        .as_ref()
        .unwrap()
        .audit
        .record_native_logout_after_refresh_closed(
            result(
                prefix.logout.clone(),
                begin.clone(),
                Outcome::Succeeded,
                Reason::UserRequested,
                Some(fixture.binding.principal.clone()),
            ),
            fixture.binding.clone(),
            prefix.refresh.operation_id,
            prefix.refresh_intention.clone(),
            refresh_result.clone(),
        )
        .unwrap()
}
async fn pending(fixture: &Fixture) {
    let reader = ReaderToken {
        raw: fixture.raw.clone(),
        binding: fixture.binding.clone(),
    };
    assert_eq!(begin(fixture, &reader).await.0, StatusCode::OK);
}

#[tokio::test]
async fn lost_ack_at_each_real_ledger_boundary_replays_the_same_three_facts() {
    for cut in 0..3 {
        let fixture = fixture().await;
        let oidc = fixture.state.oidc.as_ref().unwrap();
        assert_eq!(
            oidc.audit.format_version().unwrap(),
            4,
            "setup: composed Core V4 capacity plan required"
        );
        assert_eq!(
            oidc.audit.verify().unwrap().records,
            4,
            "setup: genesis + plan + initial intention + admission"
        );
        pending(&fixture).await;
        let records = oidc.audit.verify().unwrap().records;
        let prefix = prepare(&fixture);
        assert_eq!(prefix.logout.action, Action::Logout);
        assert_eq!(prefix.logout.phase, Phase::Intention);
        assert!(prefix.logout.causal.is_none() && prefix.logout.transaction_id.is_none());
        assert_ne!(prefix.logout.operation_id, prefix.refresh.operation_id);
        let begin_ack = begin_ledger(&fixture, &prefix);
        let mut refresh_ack = None;
        let mut end_ack = None;
        if cut >= 1 {
            native_logout_state(
                &mut oidc.registry.lock().unwrap(),
                &fixture.binding,
                prefix.refresh.operation_id,
                &prefix.refresh_intention,
            )
            .unwrap()
            .begin = Some(begin_ack.clone());
            refresh_ack = Some(close_ledger(&fixture, &prefix));
        }
        if cut >= 2 {
            native_logout_state(
                &mut oidc.registry.lock().unwrap(),
                &fixture.binding,
                prefix.refresh.operation_id,
                &prefix.refresh_intention,
            )
            .unwrap()
            .refresh_result = refresh_ack.clone();
            end_ack = Some(end_ledger(
                &fixture,
                &prefix,
                &begin_ack,
                refresh_ack.as_ref().unwrap(),
            ));
        }
        // The last real ACK is deliberately not installed in the keeper. It
        // must be observed again, never inferred from an absent promise.
        assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 1);
        assert!(oidc.authenticate(&fixture.raw).is_err());
        assert_eq!(oidc.audit.verify().unwrap().records, records + cut + 1);
        let (status, reply) = logout_request(&fixture).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["audit"], "recorded");
        assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
        assert_eq!(oidc.audit.verify().unwrap().records, records + 3);
        assert_eq!(begin_ledger(&fixture, &prefix), begin_ack);
        if let Some(ack) = refresh_ack.as_ref() {
            assert_eq!(close_ledger(&fixture, &prefix), *ack);
        }
        if let Some(ack) = end_ack {
            assert_eq!(
                end_ledger(&fixture, &prefix, &begin_ack, refresh_ack.as_ref().unwrap()),
                ack
            );
        }
        assert_eq!(oidc.audit.verify().unwrap().records, records + 3);
    }
}

#[tokio::test]
async fn end_audit_failure_retains_closed_original_until_reserved_terminal_ack() {
    let fixture = fixture_with_record_budget(7).await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    assert_eq!(oidc.audit.format_version().unwrap(), 3);
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        3,
        "setup: real initial admission has three records"
    );
    pending(&fixture).await;
    let prefix = prepare(&fixture);
    let begin_ack = begin_ledger(&fixture, &prefix);
    let refresh_ack = close_ledger(&fixture, &prefix);
    {
        let mut registry = oidc.registry.lock().unwrap();
        let state = native_logout_state(
            &mut registry,
            &fixture.binding,
            prefix.refresh.operation_id,
            &prefix.refresh_intention,
        )
        .unwrap();
        state.begin = Some(begin_ack.clone());
        state.refresh_result = Some(refresh_ack.clone());
    }
    assert_eq!(oidc.audit.verify().unwrap().records, 6);
    let lock = lock_audit_writer(&fixture);
    assert_eq!(
        logout_request(&fixture).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    {
        let mut registry = oidc.registry.lock().unwrap();
        assert_eq!(native_owners(&registry).count(), 1);
        let state = native_logout_state(
            &mut registry,
            &fixture.binding,
            prefix.refresh.operation_id,
            &prefix.refresh_intention,
        )
        .unwrap();
        assert_eq!(state.begin.as_ref(), Some(&begin_ack));
        assert_eq!(state.refresh_result.as_ref(), Some(&refresh_ack));
        assert!(state.end.is_none());
    }
    assert!(oidc.authenticate(&fixture.raw).is_err());
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(oidc.audit.verify().unwrap().records, 6);
    let deadline = oidc.registry.lock().unwrap().sessions[&fixture.binding.session_id.to_string()]
        .pending_refresh
        .as_ref()
        .unwrap()
        .deadline;
    reap_native_at(oidc, &mut oidc.registry.lock().unwrap(), deadline).unwrap();
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        7,
        "FLogout consumes its reserved L at exact capacity"
    );
    assert_eq!(begin_ledger(&fixture, &prefix), begin_ack);
    assert_eq!(oidc.audit.verify().unwrap().records, 7);
}

#[tokio::test]
async fn begin_reservation_refusal_keeps_the_original_owner_and_logout_uuid() {
    let fixture = fixture_with_record_budget(6).await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    assert_eq!(oidc.audit.format_version().unwrap(), 3);
    assert_eq!(oidc.audit.verify().unwrap().records, 3);
    pending(&fixture).await;
    let records = oidc.audit.verify().unwrap().records;
    assert_eq!(records, 4);
    let consumed = start_count(oidc);
    assert_eq!(
        logout_request(&fixture).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let operation = {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(native_owners(&registry).count(), 1);
        let pending = registry.sessions[&fixture.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        let state = pending.native_logout.as_ref().unwrap();
        assert!(state.begin.is_none() && state.refresh_result.is_none() && state.end.is_none());
        state.intention.operation_id
    };
    assert_eq!(
        logout_request(&fixture).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    {
        let registry = oidc.registry.lock().unwrap();
        let pending = registry.sessions[&fixture.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        assert_eq!(
            pending
                .native_logout
                .as_ref()
                .unwrap()
                .intention
                .operation_id,
            operation
        );
        assert!(matches!(
            oidc.audit
                .session_current(fixture.binding.session_id)
                .unwrap()
                .unwrap()
                .state,
            SessionState::RefreshPending { .. }
        ));
    }
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    assert_eq!(start_count(oidc), consumed);
    assert!(oidc.authenticate(&fixture.raw).is_err());
}

#[tokio::test]
async fn logout_preserves_an_earlier_terminal_issue_and_audits_user_request_separately() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut operation = begin_refresh(
        oidc,
        &fixture.raw,
        fixture.binding.session_id,
        fixture.binding.generation,
        SessionKind::NativeCli,
        None,
    )
    .unwrap();
    let records = oidc.audit.verify().unwrap().records;
    let lock = lock_audit_writer(&fixture);
    assert_eq!(
        fail_refresh(oidc, &operation, Outcome::Rejected, Reason::InvalidResponse)
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    lock.execute_batch("ROLLBACK").unwrap();
    let (status, reply) = logout_request(&fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["audit"], "recorded");
    operation.cancel_on_drop = false;
    assert_eq!(oidc.audit.verify().unwrap().records, records + 3);
    assert_eq!(
        oidc.audit
            .session_current(fixture.binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        SessionState::Closed {
            reason: Reason::InvalidResponse
        }
    );
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
}
