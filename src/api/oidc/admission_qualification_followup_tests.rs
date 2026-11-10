//! Private qualification draft against the immutable Gateway R2 composition.
//! No product clock or counter is rewritten. The rate parent is a pure budget
//! transition oracle, not HTTP admission or a measurement of physical time.
use super::*;

fn capacity_refusal(candidate: ApiResult<StartWindow>) {
    let error = match candidate {
        Ok(_) => panic!("a ninth start borrowed the other class's unused allowance"),
        Err(error) => error,
    };
    assert_eq!(error.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.1.0["error"]["code"], "capacity_exhausted");
    assert_eq!(error.1.0["error"]["retry_action"], "retry");
}

#[test]
fn one_sided_rate_eight_refuses_ninth_without_borrowing_the_empty_class() {
    assert_eq!(ANONYMOUS_STARTS, 8);
    assert_eq!(NATIVE_STARTS, 8);
    assert_eq!(STARTS_PER_WINDOW, 16);
    let now = Instant::now();
    let principals: Vec<_> = (0..9)
        .map(|index| {
            PrincipalRefV1::HumanOidc {
                issuer: "https://synthetic.invalid".into(),
                subject: format!("budget-unit-{index}"),
            }
            .key()
            .unwrap()
        })
        .collect();

    // Eight successful calls of the actual policy transition build each
    // window. These principal keys exercise budget arithmetic only; they do
    // not constitute authenticated sessions or bypass the Router proof gate.
    let mut anonymous = Registry::default();
    for admitted in 1..=8 {
        anonymous.rate = Some(next_window(&anonymous, None, now).unwrap());
        let window = anonymous.rate.as_ref().unwrap();
        assert_eq!(window.started, now);
        assert_eq!(u32::from(window.anonymous), admitted);
        assert!(window.authenticated.is_empty());
        assert_eq!(window.total(), admitted);
    }
    capacity_refusal(next_window(&anonymous, None, now));
    let window = anonymous.rate.as_ref().unwrap();
    assert_eq!(window.anonymous, 8);
    assert!(window.authenticated.is_empty());
    assert_eq!(window.total(), 8);
    let separate_class = next_window(&anonymous, Some(&principals[0]), now).unwrap();
    assert_eq!(separate_class.anonymous, 8);
    assert_eq!(separate_class.authenticated[&principals[0]], 1);
    assert_eq!(separate_class.total(), 9);

    let mut native = Registry::default();
    for (index, principal) in principals.iter().take(8).enumerate() {
        native.rate = Some(next_window(&native, Some(principal), now).unwrap());
        let window = native.rate.as_ref().unwrap();
        assert_eq!(window.started, now);
        assert_eq!(window.anonymous, 0);
        assert_eq!(window.authenticated.len(), index + 1);
        assert!(window.authenticated.values().all(|count| *count == 1));
        assert_eq!(window.total(), (index + 1) as u32);
    }
    capacity_refusal(next_window(&native, Some(&principals[8]), now));
    let window = native.rate.as_ref().unwrap();
    assert_eq!(window.anonymous, 0);
    assert_eq!(window.authenticated.len(), 8);
    assert_eq!(window.total(), 8);
    let separate_class = next_window(&native, None, now).unwrap();
    assert_eq!(separate_class.anonymous, 1);
    assert_eq!(separate_class.authenticated, window.authenticated);
    assert_eq!(separate_class.total(), 9);
}

fn keep_real_prefix_acks(
    fixture: &Fixture,
    prefix: &Prefix,
    begin: &AuthenticationReceiptV1,
    refresh: Option<&AuthenticationReceiptV1>,
) {
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut registry = oidc.registry.lock().unwrap();
    let state = native_logout_state(
        &mut registry,
        &fixture.binding,
        prefix.refresh.operation_id,
        &prefix.refresh_intention,
    )
    .unwrap();
    assert_eq!(state.intention.operation_id, prefix.logout.operation_id);
    assert!(state.begin.is_none() && state.refresh_result.is_none() && state.end.is_none());
    // Only ACKs just returned by real Core calls are installed, using the same
    // boundary seam as the existing R2 lost-ACK parent. No receipt is invented.
    state.begin = Some(begin.clone());
    state.refresh_result = refresh.cloned();
}

fn exact_three_facts_and_retired(
    fixture: &Fixture,
    prefix: &Prefix,
    records_before_logout: u64,
    original_begin: &AuthenticationReceiptV1,
    original_refresh: Option<&AuthenticationReceiptV1>,
) {
    let oidc = fixture.state.oidc.as_ref().unwrap();
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        records_before_logout + 3
    );
    {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(native_owners(&registry).count(), 0);
        assert!(
            !registry
                .sessions
                .contains_key(&fixture.binding.session_id.to_string())
        );
        assert!(registry.native.values().all(|intent| {
            intent.previous.as_ref() != Some(&fixture.binding)
                || intent.operation_id != prefix.refresh.operation_id
                || intent.intention.as_ref() != Some(&prefix.refresh_intention)
        }));
    }
    let current = oidc
        .audit
        .session_current(fixture.binding.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(current.binding, fixture.binding);
    assert_eq!(
        current.state,
        SessionState::Closed {
            reason: prefix.choice.reason
        }
    );
    assert!(oidc.authenticate(&fixture.raw).is_err());
    let begin = begin_ledger(fixture, prefix);
    assert_eq!(begin, *original_begin);
    let refresh = close_ledger(fixture, prefix);
    if let Some(original) = original_refresh {
        assert_eq!(refresh, *original);
    }
    let end = end_ledger(fixture, prefix, &begin, &refresh);
    assert_eq!(begin.namespace_id, refresh.namespace_id);
    assert_eq!(refresh.namespace_id, end.namespace_id);
    assert_eq!(begin.sequence + 1, refresh.sequence);
    assert_eq!(refresh.sequence + 1, end.sequence);
    assert_eq!(end_ledger(fixture, prefix, &begin, &refresh), end);
    assert_eq!(
        oidc.audit.verify().unwrap().records,
        records_before_logout + 3,
        "all three exact record/UUID/binding/receipt calls must replay without a new fact"
    );
}

#[tokio::test]
async fn valid_finish_resumes_retained_typed_logout_between_begin_and_end_without_tokens() {
    for refresh_already_closed in [false, true] {
        let fixture = fixture().await;
        let oidc = fixture.state.oidc.as_ref().unwrap();
        assert_eq!(oidc.audit.format_version().unwrap(), 4);
        assert_eq!(
            oidc.audit.verify().unwrap().records,
            4,
            "setup: genesis + V4 plan + initial intention + admission"
        );
        let reader = ReaderToken {
            raw: fixture.raw.clone(),
            binding: fixture.binding.clone(),
        };
        let (status, reply) = begin(&fixture, &reader).await;
        assert_eq!(status, StatusCode::OK);
        let id = intent(&reply);
        let records = oidc.audit.verify().unwrap().records;
        let (created, deadline) = {
            let registry = oidc.registry.lock().unwrap();
            (
                registry.native[&id].created,
                registry.sessions[&fixture.binding.session_id.to_string()]
                    .pending_refresh
                    .as_ref()
                    .unwrap()
                    .deadline,
            )
        };
        let prefix = prepare(&fixture);
        assert_eq!(prefix.refresh.operation_id, id);
        let begin_ack = begin_ledger(&fixture, &prefix);
        let refresh_ack = refresh_already_closed.then(|| close_ledger(&fixture, &prefix));
        keep_real_prefix_acks(&fixture, &prefix, &begin_ack, refresh_ack.as_ref());
        {
            let registry = oidc.registry.lock().unwrap();
            assert_eq!(native_owners(&registry).count(), 1);
            assert_eq!(registry.native[&id].created, created);
            assert_eq!(
                registry.sessions[&fixture.binding.session_id.to_string()]
                    .pending_refresh
                    .as_ref()
                    .unwrap()
                    .deadline,
                deadline
            );
        }
        assert!(oidc.authenticate(&fixture.raw).is_err());
        assert!(
            created.elapsed() < Duration::from_secs(10),
            "setup: less than five seconds of the ORIGINAL Finish TTL remain; no clock is rearmed"
        );
        let (status, reply) = tokio::time::timeout(
            Duration::from_secs(20),
            request(
                &fixture.state,
                json!({"kind":"finish", "intent_id":id, "outcome":"cancelled"}),
                Some(&fixture.raw),
            ),
        )
        .await
        .expect("bounded Finish request did not return");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reply,
            json!({"version":1,"status":"finished"}),
            "Finish acknowledges closure; no access/refresh token or rotation is delivered"
        );
        exact_three_facts_and_retired(&fixture, &prefix, records, &begin_ack, refresh_ack.as_ref());
    }
}

#[tokio::test]
async fn real_refresh_operation_drop_resumes_logout_and_failed_audit_keeps_the_exact_owner() {
    for refresh_already_closed in [false, true] {
        for audit_refused in [false, true] {
            let fixture = fixture().await;
            let oidc = fixture.state.oidc.as_ref().unwrap();
            assert_eq!(oidc.audit.format_version().unwrap(), 4);
            assert_eq!(
                oidc.audit.verify().unwrap().records,
                4,
                "setup: genesis + V4 plan + initial intention + admission"
            );
            let operation = begin_refresh(
                oidc,
                &fixture.raw,
                fixture.binding.session_id,
                fixture.binding.generation,
                SessionKind::NativeCli,
                None,
            )
            .unwrap();
            assert!(operation.inline_cleanup && operation.cancel_on_drop);
            let deadline = operation.completion_deadline.unwrap();
            let records = oidc.audit.verify().unwrap().records;
            let prefix = prepare(&fixture);
            assert_eq!(prefix.refresh.operation_id, operation.record.operation_id);
            assert_eq!(prefix.refresh_intention, operation.receipt);
            let begin_ack = begin_ledger(&fixture, &prefix);
            let refresh_ack = refresh_already_closed.then(|| close_ledger(&fixture, &prefix));
            keep_real_prefix_acks(&fixture, &prefix, &begin_ack, refresh_ack.as_ref());
            assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 1);
            let lock = audit_refused.then(|| lock_audit_writer(&fixture));
            // The real product destructor runs in its actual synchronous
            // preparation mode. No RefreshCleanup is constructed by this test.
            drop(operation);
            if let Some(lock) = lock {
                {
                    let registry = oidc.registry.lock().unwrap();
                    assert_eq!(native_owners(&registry).count(), 1);
                    let session = &registry.sessions[&fixture.binding.session_id.to_string()];
                    assert!(session.rotating);
                    assert_eq!(session.binding, fixture.binding);
                    let pending = session.pending_refresh.as_ref().unwrap();
                    assert_eq!(pending.record.operation_id, prefix.refresh.operation_id);
                    assert_eq!(pending.receipt, prefix.refresh_intention);
                    assert_eq!(pending.deadline, deadline);
                    assert_eq!(pending.native_close, Some(prefix.choice));
                    let logout = pending.native_logout.as_ref().unwrap();
                    assert_eq!(logout.intention.operation_id, prefix.logout.operation_id);
                    assert_eq!(logout.begin.as_ref(), Some(&begin_ack));
                    assert_eq!(logout.refresh_result.as_ref(), refresh_ack.as_ref());
                    assert!(logout.end.is_none());
                }
                assert!(oidc.authenticate(&fixture.raw).is_err());
                assert_eq!(
                    oidc.audit.verify().unwrap().records,
                    records + 1 + u64::from(refresh_already_closed)
                );
                lock.execute_batch("ROLLBACK").unwrap();
                // Controlled observation of the same original deadline, not
                // elapsed physical time, a new Instant or a background worker.
                reap_native_at(oidc, &mut oidc.registry.lock().unwrap(), deadline).unwrap();
            }
            exact_three_facts_and_retired(
                &fixture,
                &prefix,
                records,
                &begin_ack,
                refresh_ack.as_ref(),
            );
        }
    }
}

#[tokio::test]
async fn durable_refresh_before_first_logout_must_reconcile_without_a_retroactive_logout_begin() {
    for publish_intent in [false, true] {
        let fixture = fixture().await;
        let oidc = fixture.state.oidc.as_ref().unwrap();
        assert_eq!(oidc.audit.format_version().unwrap(), 4);
        assert_eq!(
            oidc.audit.verify().unwrap().records,
            4,
            "setup: genesis + V4 plan + initial intention + admission"
        );
        let operation = if publish_intent {
            let reader = ReaderToken {
                raw: fixture.raw.clone(),
                binding: fixture.binding.clone(),
            };
            let (status, _) = begin(&fixture, &reader).await;
            assert_eq!(status, StatusCode::OK);
            None
        } else {
            Some(
                begin_refresh(
                    oidc,
                    &fixture.raw,
                    fixture.binding.session_id,
                    fixture.binding.generation,
                    SessionKind::NativeCli,
                    None,
                )
                .unwrap(),
            )
        };
        let (operation_id, receipt, deadline) = {
            let registry = oidc.registry.lock().unwrap();
            let pending = registry.sessions[&fixture.binding.session_id.to_string()]
                .pending_refresh
                .as_ref()
                .unwrap();
            if publish_intent {
                let intent = &registry.native[&pending.record.operation_id];
                assert_eq!(intent.previous.as_ref(), Some(&fixture.binding));
                assert_eq!(intent.intention.as_ref(), Some(&pending.receipt));
            }
            (
                pending.record.operation_id,
                pending.receipt.clone(),
                pending.deadline,
            )
        };
        let other = if publish_intent {
            let reader = issue_session(&fixture, "other-published-owner", 0).await;
            let (status, reply) = begin(&fixture, &reader).await;
            assert_eq!(status, StatusCode::OK);
            let id = intent(&reply);
            let (receipt, created, other_deadline) = {
                let registry = oidc.registry.lock().unwrap();
                let intent = &registry.native[&id];
                let pending = registry.sessions[&reader.binding.session_id.to_string()]
                    .pending_refresh
                    .as_ref()
                    .unwrap();
                (pending.receipt.clone(), intent.created, pending.deadline)
            };
            assert!(
                other_deadline > deadline,
                "setup: the other owner was admitted after the original"
            );
            Some((reader, id, receipt, created, other_deadline))
        } else {
            None
        };
        let (record, choice) = retain_native_close_choice(
            &mut oidc.registry.lock().unwrap(),
            &fixture.binding,
            operation_id,
            &receipt,
            Outcome::Rejected,
            Reason::InvalidResponse,
        )
        .unwrap();
        let final_record = result(
            record,
            receipt,
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
        // Deliberately omit this genuine ACK from the original keeper. There was
        // no ILogout when FRefresh committed; no retroactive BEGIN is permissible.
        let records = oidc.audit.verify().unwrap().records;
        if publish_intent {
            let registry = oidc.registry.lock().unwrap();
            assert!(
                registry.native.contains_key(&operation_id),
                "setup: the true Router intent remains published before Logout"
            );
            assert_eq!(native_owners(&registry).count(), 2);
        }
        let current = oidc
            .audit
            .session_current(fixture.binding.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.binding, fixture.binding);
        assert_eq!(
            current.state,
            SessionState::Closed {
                reason: Reason::InvalidResponse
            }
        );
        let (status, reply) = logout_request(&fixture).await;
        assert_eq!(oidc.audit.verify().unwrap().records, records);
        assert_eq!(
            oidc.audit
                .close_session(
                    final_record.clone(),
                    fixture.binding.generation,
                    fixture.binding.access_fingerprint
                )
                .unwrap(),
            final_ack,
            "setup: strict replay can observe the SAME actual FRefresh ACK without a write"
        );
        let reap_error = reap_native_at(oidc, &mut oidc.registry.lock().unwrap(), deadline)
            .err()
            .map(|error| error.0);
        // Keep cancel_on_drop=true: this is the real destructor, unlike the
        // historical R2 parent that disabled Drop after checking repeated 503s.
        drop(operation);
        assert_eq!(
            oidc.audit.verify().unwrap().records,
            records,
            "no new ILogout/END or unaudited successful Logout may be invented"
        );
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
        let current = oidc
            .audit
            .session_current(fixture.binding.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.binding, fixture.binding);
        assert_eq!(
            current.state,
            SessionState::Closed {
                reason: Reason::InvalidResponse
            }
        );
        assert!(oidc.authenticate(&fixture.raw).is_err());
        {
            let registry = oidc.registry.lock().unwrap();
            assert_eq!(
                native_owners(&registry).count(),
                usize::from(other.is_some()),
                "exact durable FRefresh must be reconcilable without pinning a volatile owner; published={publish_intent}, logout status={status:?}, reap status={reap_error:?}"
            );
            assert!(
                !registry
                    .sessions
                    .contains_key(&fixture.binding.session_id.to_string())
            );
            assert!(
                !registry.native.contains_key(&operation_id),
                "retire the exact published intent with its original Pending"
            );
            if let Some((reader, id, receipt, created, deadline)) = other.as_ref() {
                let intent = &registry.native[id];
                assert_eq!(intent.previous.as_ref(), Some(&reader.binding));
                assert_eq!(intent.operation_id, *id);
                assert_eq!(intent.intention.as_ref(), Some(receipt));
                assert_eq!(intent.created, *created);
                let pending = registry.sessions[&reader.binding.session_id.to_string()]
                    .pending_refresh
                    .as_ref()
                    .unwrap();
                assert_eq!(pending.record.operation_id, *id);
                assert_eq!(pending.receipt, *receipt);
                assert_eq!(pending.deadline, *deadline);
                assert!(pending.native_close.is_none() && pending.native_logout.is_none());
            }
        }
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reply,
            json!({"version":1,"local_session":"unknown",
        "provider_logout":"not_attempted","audit":"unavailable"}),
            "reconciliation of FRefresh is not a journaled Logout success"
        );
        if let Some((reader, id, _, _, _)) = other {
            finish(&fixture, &reader, id).await;
            assert_eq!(
                oidc.audit.verify().unwrap().records,
                records + 1,
                "the separate owner closes only on its own explicit Finish"
            );
        }
    }
}

#[tokio::test]
async fn current_closed_without_the_exact_original_keeper_proof_never_releases_the_owner() {
    for divergence in 0..4 {
        let fixture = fixture().await;
        let oidc = fixture.state.oidc.as_ref().unwrap();
        assert_eq!(oidc.audit.format_version().unwrap(), 4);
        assert_eq!(
            oidc.audit.verify().unwrap().records,
            4,
            "setup: genesis + V4 plan + initial intention + admission"
        );
        let operation = begin_refresh(
            oidc,
            &fixture.raw,
            fixture.binding.session_id,
            fixture.binding.generation,
            SessionKind::NativeCli,
            None,
        )
        .unwrap();
        let (record, choice) = retain_native_close_choice(
            &mut oidc.registry.lock().unwrap(),
            &fixture.binding,
            operation.record.operation_id,
            &operation.receipt,
            Outcome::Rejected,
            Reason::InvalidResponse,
        )
        .unwrap();
        let final_record = result(
            record,
            operation.receipt.clone(),
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
        let current = oidc
            .audit
            .session_current(fixture.binding.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.binding, fixture.binding);
        assert_eq!(
            current.state,
            SessionState::Closed {
                reason: Reason::InvalidResponse
            }
        );
        let records = oidc.audit.verify().unwrap().records;
        {
            let mut registry = oidc.registry.lock().unwrap();
            let session = registry
                .sessions
                .get_mut(&fixture.binding.session_id.to_string())
                .unwrap();
            let pending = session.pending_refresh.as_mut().unwrap();
            assert!(pending.native_logout.is_none());
            // Deliberate corruption of the existing synthetic keeper. No
            // fabricated Session or public admission is used. These cases
            // have the SAME genuine CurrentClosed, which cannot prove release.
            match divergence {
                0 => pending.receipt = final_ack.clone(),
                1 => {
                    pending.native_close = Some(NativeCloseChoice {
                        outcome: Outcome::Rejected,
                        reason: Reason::Revoked,
                    })
                }
                2 => session.binding.expires_at_unix_seconds += 1,
                _ => {}
            }
        }
        // The fourth case keeps the exact original proof, but prevents the
        // actual replay ACK. QueryClosed must not release this owner either.
        let lock = (divergence == 3).then(|| lock_audit_writer(&fixture));
        let (status, reply) = logout_request(&fixture).await;
        {
            let registry = oidc.registry.lock().unwrap();
            assert_eq!(
                native_owners(&registry).count(),
                1,
                "CurrentClosed is not an ACK for divergent keeper case {divergence}"
            );
            let session = &registry.sessions[&fixture.binding.session_id.to_string()];
            assert!(session.rotating);
            assert!(
                session
                    .pending_refresh
                    .as_ref()
                    .unwrap()
                    .native_logout
                    .is_none(),
                "a refused reconciliation must not prepare a new ILogout/UUID"
            );
        }
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(reply["error"]["code"], "audit_unavailable");
        assert_eq!(reply["error"]["retry_action"], "contact_operator");
        assert_eq!(oidc.audit.verify().unwrap().records, records);
        if let Some(lock) = lock {
            lock.execute_batch("ROLLBACK").unwrap();
        }
        drop(operation);
        assert_eq!(
            native_owners(&oidc.registry.lock().unwrap()).count(),
            usize::from(divergence != 3),
            "only the exact owner with a newly available real replay ACK may retire"
        );
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
    }
}
