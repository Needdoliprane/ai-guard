//! Real verified sessions and durable transitions; no Session is fabricated.
use super::*;

#[path = "admission_capacity_faults.rs"]
mod faults;

struct ReaderToken {
    raw: String,
    binding: SessionBinding,
}
async fn issue_session(fixture: &Fixture, subject: &str, salt: usize) -> ReaderToken {
    let principal = PrincipalRefV1::HumanOidc {
        issuer: fixture._issuer.issuer.clone(),
        subject: subject.into(),
    };
    for tenant in fixture.state.tenants.iter() {
        if tenant.store.human_account(&principal).unwrap().is_none() {
            tenant
                .store
                .mutate_identity(&identity_change(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Synthetic budget reader".into(),
                        revision: 1,
                        active: true,
                    },
                }))
                .unwrap();
            tenant
                .store
                .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        tenant_id: tenant.tenant_id.clone(),
                        organization_scope: "organization-ab".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                    },
                }))
                .unwrap();
        }
    }
    let raw = fixture._issuer.token(
        "cli-public",
        subject,
        None,
        &[],
        &format!("budget-{subject}-{salt}"),
    );
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let access = oidc.provider.verify_access(&raw).await.unwrap();
    let installed = raw.clone();
    let binding = fixture
        .state
        .blocking_identity(move |state| {
            let oidc = configured(&state)?;
            let record = record(
                &oidc,
                Action::SessionAdmission,
                Uuid::new_v4(),
                "cli-public",
                None,
                None,
                Uuid::new_v4(),
            );
            install(
                &state,
                &oidc,
                NewAdmission {
                    record,
                    access,
                    raw: &installed,
                    refresh: None,
                    logout_context: None,
                    kind: SessionKind::NativeCli,
                    browser_transaction: None,
                },
            )
        })
        .await
        .unwrap();
    oidc.authenticate(&raw).unwrap();
    ReaderToken { raw, binding }
}
async fn begin(fixture: &Fixture, reader: &ReaderToken) -> (StatusCode, Value) {
    request(
        &fixture.state,
        json!({"kind":"refresh", "authentication_id":reader.binding.session_id,
        "generation":reader.binding.generation}),
        Some(&reader.raw),
    )
    .await
}
fn intent(reply: &Value) -> Uuid {
    Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap()
}
async fn finish(fixture: &Fixture, reader: &ReaderToken, id: Uuid) {
    let (status, _) = request(
        &fixture.state,
        json!({"kind":"finish", "intent_id":id, "outcome":"cancelled"}),
        Some(&reader.raw),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "setup: exact pending operation must durably close"
    );
}

#[tokio::test]
async fn per_session_and_principal_rate_reject_without_a_second_durable_admission() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let first = ReaderToken {
        raw: fixture.raw.clone(),
        binding: fixture.binding.clone(),
    };
    let second = issue_session(&fixture, "synthetic-capacity-reader", 1).await;
    let third = issue_session(&fixture, "synthetic-capacity-reader", 2).await;
    reset_start_window(oidc);
    let (status, reply) = begin(&fixture, &first).await;
    assert_eq!(status, StatusCode::OK);
    let first_id = intent(&reply);
    let records = oidc.audit.verify().unwrap().records;
    assert_eq!(begin(&fixture, &first).await.0, StatusCode::CONFLICT);
    assert_eq!(start_count(oidc), 1);
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    let (status, reply) = begin(&fixture, &second).await;
    assert_eq!(status, StatusCode::OK);
    let second_id = intent(&reply);
    assert_eq!(
        start_count(oidc),
        2,
        "setup: both starts must share the real window"
    );
    let records = oidc.audit.verify().unwrap().records;
    assert!(
        oidc.registry
            .lock()
            .unwrap()
            .rate
            .as_ref()
            .unwrap()
            .started
            .elapsed()
            < Duration::from_secs(1),
        "setup: principal rate window rolled over"
    );
    let (status, reply) = begin(&fixture, &third).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply["error"]["code"], "capacity_exhausted");
    assert_eq!(start_count(oidc), 2);
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    finish(&fixture, &first, first_id).await;
    finish(&fixture, &second, second_id).await;
}

#[tokio::test]
async fn both_rate_classes_share_one_window_with_eight_verified_principal_keys() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut readers = Vec::new();
    for index in 0..9 {
        readers.push(issue_session(&fixture, &format!("rate-reader-{index}"), 0).await);
    }
    reset_start_window(oidc);
    for _ in 0..8 {
        assert_eq!(
            request(&fixture.state, json!({"kind":"login"}), None)
                .await
                .0,
            StatusCode::OK
        );
    }
    retime_full_anonymous_window(oidc);
    let mut pending = Vec::new();
    for reader in readers.iter().take(8) {
        let (status, reply) = begin(&fixture, reader).await;
        assert_eq!(status, StatusCode::OK);
        pending.push(intent(&reply));
    }
    {
        let registry = oidc.registry.lock().unwrap();
        let rate = registry.rate.as_ref().unwrap();
        assert_eq!(rate.anonymous, 8, "setup: shared rate window rolled over");
        assert_eq!(rate.authenticated.len(), 8);
        assert_eq!(rate.total(), 16);
        assert!(
            rate.started.elapsed() < Duration::from_secs(1),
            "setup: shared window rolled over"
        );
    }
    assert_eq!(
        request(&fixture.state, json!({"kind":"login"}), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        begin(&fixture, &readers[8]).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    for (reader, id) in readers.iter().zip(pending) {
        finish(&fixture, reader, id).await;
    }
}

#[tokio::test]
async fn active_owners_preserve_both_slot_partitions_and_principal_four_without_loans() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut readers = Vec::new();
    for principal in 0..8 {
        for session in 0..4 {
            readers.push(issue_session(&fixture, &format!("slots-{principal}"), session).await);
        }
    }
    let fifth = issue_session(&fixture, "slots-0", 5).await;
    let outside = issue_session(&fixture, "slots-outside", 0).await;
    for _ in 0..224 {
        reset_start_window(oidc);
        assert_eq!(
            request(&fixture.state, json!({"kind":"login"}), None)
                .await
                .0,
            StatusCode::OK
        );
    }
    let mut pending = Vec::new();
    for (index, reader) in readers.iter().enumerate() {
        reset_start_window(oidc);
        let (status, reply) = begin(&fixture, reader).await;
        assert_eq!(status, StatusCode::OK);
        pending.push(intent(&reply));
        assert_eq!(
            native_owners(&oidc.registry.lock().unwrap()).count(),
            index + 1,
            "setup: original fifteen-second owner expired during slot preparation"
        );
        if index == 3 {
            reset_start_window(oidc);
            let records = oidc.audit.verify().unwrap().records;
            assert_eq!(
                begin(&fixture, &fifth).await.0,
                StatusCode::TOO_MANY_REQUESTS
            );
            assert_eq!(start_count(oidc), 0);
            assert_eq!(oidc.audit.verify().unwrap().records, records);
        }
    }
    assert_eq!(anonymous_slots(&oidc.registry.lock().unwrap()), 224);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 32);
    reset_start_window(oidc);
    assert_eq!(
        begin(&fixture, &outside).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        request(&fixture.state, json!({"kind":"login"}), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    finish(&fixture, &readers[0], pending[0]).await;
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 31);
    assert_eq!(
        request(&fixture.state, json!({"kind":"login"}), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (status, reply) = begin(&fixture, &outside).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 32);
    finish(&fixture, &outside, intent(&reply)).await;
    for (reader, id) in readers.iter().zip(pending).skip(1) {
        finish(&fixture, reader, id).await;
    }
}

#[tokio::test]
async fn invalid_generation_and_durable_revocation_leave_rate_and_admission_unchanged() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let records = oidc.audit.verify().unwrap().records;
    assert_eq!(
        request(
            &fixture.state,
            json!({"kind":"refresh", "authentication_id":fixture.binding.session_id,
        "generation":fixture.binding.generation + 1}),
            Some(&fixture.raw)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(start_count(oidc), 0);
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    {
        let registry = oidc.registry.lock().unwrap();
        close_record(
            oidc,
            &registry.sessions[&fixture.binding.session_id.to_string()],
            Action::Logout,
            Reason::Revoked,
        )
        .unwrap();
    }
    let records = oidc.audit.verify().unwrap().records;
    assert_eq!(
        request(
            &fixture.state,
            json!({"kind":"refresh", "authentication_id":fixture.binding.session_id,
        "generation":fixture.binding.generation}),
            Some(&fixture.raw)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(start_count(oidc), 0);
    assert_eq!(oidc.audit.verify().unwrap().records, records);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
}

#[tokio::test]
async fn pending_owner_bridges_intent_absence_until_real_verified_rotation_commits() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let (created, maximum) = {
        let registry = oidc.registry.lock().unwrap();
        let session = &registry.sessions[&fixture.binding.session_id.to_string()];
        (session.created, session.maximum_expiry)
    };
    let mut operation = begin_refresh(
        oidc,
        &fixture.raw,
        fixture.binding.session_id,
        fixture.binding.generation,
        SessionKind::NativeCli,
        None,
    )
    .unwrap();
    assert!(oidc.registry.lock().unwrap().native.is_empty());
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 1);
    let next = fixture._issuer.token(
        "cli-public",
        "synthetic-capacity-reader",
        None,
        &[],
        "actual-next-generation",
    );
    let access = oidc.provider.verify_access(&next).await.unwrap();
    let binding = install_rotation(
        &fixture.state,
        oidc,
        &operation,
        RotatedAccess {
            access,
            raw: &next,
            refresh: None,
            logout: None,
        },
    )
    .unwrap();
    operation.cancel_on_drop = false;
    assert_eq!(binding.generation, fixture.binding.generation + 1);
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    let registry = oidc.registry.lock().unwrap();
    let session = &registry.sessions[&binding.session_id.to_string()];
    assert_eq!(session.created, created);
    assert_eq!(session.maximum_expiry, maximum);
    assert!(session.proof.deadline <= created + Duration::from_secs(3600));
    drop(registry);
    assert!(oidc.authenticate(&fixture.raw).is_err());
    assert_eq!(
        oidc.authenticate(&next).unwrap().generation,
        binding.generation
    );
}
