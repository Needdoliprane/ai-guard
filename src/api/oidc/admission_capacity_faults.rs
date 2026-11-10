//! Real SQLite writer contention; original native owners and receipts survive.
use super::*;

#[path = "admission_native_logout_tests.rs"]
mod native_logout;

async fn logout_request(fixture: &Fixture) -> (StatusCode, Value) {
    let response = gateway_router(fixture.state.clone()).oneshot(Request::post("/auth/oidc/logout")
        .header("content-type", "application/json").header("authorization", format!("Bearer {}", fixture.raw))
        .body(Body::from(serde_json::to_vec(&json!({"version":1,
            "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation})).unwrap())).unwrap())
        .await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 35_000)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn pending_native_logout_keeps_owner_on_audit_failure_then_closes_original_and_replay_is_unknown()
 {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let reader = ReaderToken {
        raw: fixture.raw.clone(),
        binding: fixture.binding.clone(),
    };
    let (status, reply) = begin(&fixture, &reader).await;
    assert_eq!(status, StatusCode::OK);
    let id = intent(&reply);
    let records = oidc.audit.verify().unwrap().records;
    let lock = lock_audit_writer(&fixture);
    assert_eq!(
        logout_request(&fixture).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (logout_intention, refresh_intention) = {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(native_owners(&registry).count(), 1);
        let pending = registry.sessions[&fixture.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        assert_eq!(pending.record.operation_id, id);
        assert_eq!(
            pending.native_close,
            Some(NativeCloseChoice {
                outcome: Outcome::Rejected,
                reason: Reason::Revoked
            })
        );
        assert_eq!(
            registry.native[&id].intention.as_ref(),
            Some(&pending.receipt)
        );
        let logout = pending.native_logout.as_ref().unwrap();
        assert_eq!(logout.intention.action, Action::Logout);
        assert_eq!(logout.intention.phase, Phase::Intention);
        assert!(logout.intention.transaction_id.is_none() && logout.intention.causal.is_none());
        assert_ne!(logout.intention.operation_id, id);
        assert!(logout.begin.is_none() && logout.refresh_result.is_none() && logout.end.is_none());
        (logout.intention.clone(), pending.receipt.clone())
    };
    assert!(oidc.authenticate(&fixture.raw).is_err());
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    let (status, reply) = logout_request(&fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"version":1,"local_session":"closed","provider_logout":"not_attempted","audit":"recorded"})
    );
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        records + 3,
        "typed ILogout + original FRefresh + typed FLogout must all be durable"
    );
    assert_eq!(
        oidc.audit
            .session_current(fixture.binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        SessionState::Closed {
            reason: Reason::Revoked
        }
    );
    let (status, reply) = logout_request(&fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply["local_session"], "unknown",
        "absence is not a new completion witness"
    );
    assert_eq!(reply["audit"], "unavailable");
    assert_eq!(oidc.audit.verify().unwrap().records, records + 3);
    oidc.audit
        .begin_native_logout_during_refresh(
            logout_intention,
            fixture.binding.clone(),
            id,
            refresh_intention,
        )
        .unwrap();
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        records + 3,
        "same original BEGIN replays after END"
    );
}

#[tokio::test]
async fn original_native_deadline_is_observed_without_rearming_or_changing_any_clock() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let reader = ReaderToken {
        raw: fixture.raw.clone(),
        binding: fixture.binding.clone(),
    };
    let (status, reply) = begin(&fixture, &reader).await;
    assert_eq!(status, StatusCode::OK);
    let id = intent(&reply);
    let deadline = oidc.registry.lock().unwrap().sessions[&fixture.binding.session_id.to_string()]
        .pending_refresh
        .as_ref()
        .unwrap()
        .deadline;
    let records = oidc.audit.verify().unwrap().records;
    {
        let mut registry = oidc.registry.lock().unwrap();
        reap_native_at(oidc, &mut registry, deadline - Duration::from_nanos(1)).unwrap();
        assert_eq!(native_owners(&registry).count(), 1);
        assert_eq!(
            registry.sessions[&fixture.binding.session_id.to_string()]
                .pending_refresh
                .as_ref()
                .unwrap()
                .deadline,
            deadline
        );
        assert!(registry.native.contains_key(&id));
    }
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    reap_native_at(oidc, &mut oidc.registry.lock().unwrap(), deadline).unwrap();
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert!(!oidc.registry.lock().unwrap().native.contains_key(&id));
    assert_eq!(oidc.audit.verify().unwrap().records, records + 1);
    let consumed = start_count(oidc);
    assert_eq!(begin(&fixture, &reader).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(start_count(oidc), consumed);
}

fn lock_audit_writer(fixture: &Fixture) -> rusqlite::Connection {
    // Contention in the synthetic fixture, not a success mock or integrity
    // bypass. The product's five-second SQLite busy bound stays unchanged.
    let lock =
        rusqlite::Connection::open(fixture._files.path().join("authentication.sqlite")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    lock
}

#[tokio::test]
async fn audit_admission_refusal_does_not_commit_a_rate_window_or_pending_owner() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let records = oidc.audit.verify().unwrap().records;
    let lock = lock_audit_writer(&fixture);
    let (status, reply) = request(
        &fixture.state,
        json!({"kind":"refresh",
        "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation}),
        Some(&fixture.raw),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply["error"]["code"], "audit_unavailable");
    assert_eq!(start_count(oidc), 0);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert!(
        !oidc.registry.lock().unwrap().sessions[&fixture.binding.session_id.to_string()].rotating
    );
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    refresh_and_finish(&fixture).await;
}

#[tokio::test]
async fn failed_close_keeps_exact_owner_and_first_issue_through_observed_deadline_cleanup() {
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
    let original_deadline = operation.completion_deadline.unwrap();
    let records = oidc.audit.verify().unwrap().records;
    let lock = lock_audit_writer(&fixture);
    assert_eq!(
        fail_refresh(oidc, &operation, Outcome::Rejected, Reason::InvalidResponse)
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(native_owners(&registry).count(), 1);
        let pending = registry.sessions[&fixture.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        assert_eq!(pending.deadline, original_deadline);
        assert_eq!(pending.receipt, operation.receipt);
        assert_eq!(pending.record.operation_id, operation.record.operation_id);
        assert_eq!(
            pending.native_close,
            Some(NativeCloseChoice {
                outcome: Outcome::Rejected,
                reason: Reason::InvalidResponse
            })
        );
    }
    assert!(oidc.authenticate(&fixture.raw).is_err());
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    // Observe the ORIGINAL deadline. No Session, operation, clock or rate field
    // is rewritten; this does not measure a physical expiration.
    reap_native_at(oidc, &mut oidc.registry.lock().unwrap(), original_deadline).unwrap();
    operation.cancel_on_drop = false;
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
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
    assert_eq!(oidc.audit.verify().unwrap().records, records + 1);
    let consumed = start_count(oidc);
    assert_eq!(
        request(
            &fixture.state,
            json!({"kind":"refresh",
        "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation}),
            Some(&fixture.raw)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(start_count(oidc), consumed);
    assert_eq!(oidc.audit.verify().unwrap().records, records + 1);
}

#[tokio::test]
async fn failed_finish_retains_both_exact_owners_and_divergence_refuses_without_audit() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let reader = ReaderToken {
        raw: fixture.raw.clone(),
        binding: fixture.binding.clone(),
    };
    let (status, reply) = begin(&fixture, &reader).await;
    assert_eq!(status, StatusCode::OK);
    let id = intent(&reply);
    let records = oidc.audit.verify().unwrap().records;
    let lock = lock_audit_writer(&fixture);
    let finish_action = json!({"kind":"finish", "intent_id":id, "outcome":"cancelled"});
    assert_eq!(
        request(&fixture.state, finish_action.clone(), Some(&fixture.raw))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let original = NativeCloseChoice {
        outcome: Outcome::Unknown,
        reason: Reason::Interrupted,
    };
    {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(
            native_owners(&registry).count(),
            1,
            "session + intent represent one operation"
        );
        assert_eq!(registry.native[&id].native_close, Some(original));
        assert_eq!(
            registry.sessions[&fixture.binding.session_id.to_string()]
                .pending_refresh
                .as_ref()
                .unwrap()
                .native_close,
            Some(original)
        );
    }
    assert!(oidc.authenticate(&fixture.raw).is_err());
    let provider_requests = fixture._issuer.requests().len();
    let response = gateway_router(fixture.state.clone())
        .oneshot(
            Request::post("/auth/oidc/rotate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", fixture.raw))
                .body(Body::from(
                    serde_json::to_vec(&json!({"version":1,"intent_id":id,
            "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation,
            "access_token":"synthetic-unverified-next-token"}))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(fixture._issuer.requests().len(), provider_requests);
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    {
        // Contradict only a closed enum in the existing real intent. No proof,
        // identity or owner is fabricated, removed or made usable again.
        let mut registry = oidc.registry.lock().unwrap();
        registry.native.get_mut(&id).unwrap().native_close = Some(NativeCloseChoice {
            outcome: Outcome::Rejected,
            reason: Reason::ProviderRejected,
        });
    }
    assert_eq!(
        request(&fixture.state, finish_action, Some(&fixture.raw))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 1);
    // Repair only this injected test metadata to the still-retained first
    // choice for cleanup. There is no product reconciliation/reset API here.
    oidc.registry
        .lock()
        .unwrap()
        .native
        .get_mut(&id)
        .unwrap()
        .native_close = Some(original);
    finish(&fixture, &reader, id).await;
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    assert_eq!(oidc.audit.verify().unwrap().records, records + 1);
}
