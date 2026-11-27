//! D5 follow-up: real Router producers and disjoint verified memberships.
//! The anonymous setup resets only its private rate window. The rate oracle
//! uses one fixed instant and verified session memberships. The owner
//! scenarios use the original 15s deadlines.
use super::*;

struct Admitted {
    raw: String,
    binding: SessionBinding,
}

#[tokio::test]
async fn reversed_durable_plan_order_resolves_verified_membership_ids() {
    let mut plan = capacity_plan(32, 483, 483);
    plan.tenants.reverse();
    let fixture = fixture_with_plan(1000, Some(plan)).await;
    // GatewayState stores A then B; the durable plan is deliberately B then A.
    assert_eq!(fixture.state.tenants[0].tenant_id, "tenant-a");
    let oidc = fixture.state.oidc.as_ref().unwrap();
    assert_eq!(
        oidc.audit.capacity_plan().unwrap().unwrap().tenants[0].tenant_id,
        "tenant-b"
    );
    member(&fixture, "tenant-b", "reverse-b");
    let b = signed_admission(&fixture, "reverse-b", 0).await;
    member(&fixture, "tenant-a", "reverse-a");
    let a = signed_admission(&fixture, "reverse-a", 0).await;
    reset_start_window(oidc);
    let mut owners = Vec::new();
    for (reader, expected) in [(&b, "tenant-b"), (&a, "tenant-a")] {
        let (status, reply) = begin(&fixture, reader).await;
        assert_eq!(status, StatusCode::OK);
        let id = Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap();
        let registry = oidc.registry.lock().unwrap();
        let intent = &registry.native[&id];
        let pending = registry.sessions[&reader.binding.session_id.to_string()]
            .pending_refresh
            .as_ref()
            .unwrap();
        assert_eq!(intent.registry_tenant, pending.registry_tenant);
        let key = intent.registry_tenant.unwrap();
        assert_eq!(
            registry.tenant_limits.as_ref().unwrap().tenant_id(key),
            Some(expected)
        );
        assert_eq!(
            registry
                .rate
                .as_ref()
                .and_then(|window| window.tenants.get(&key)),
            Some(&1),
            "native admission must charge its verified tenant"
        );
        owners.push((reader, id));
    }
    for (reader, id) in owners {
        finish(&fixture, reader, id).await;
    }
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
}

#[tokio::test]
async fn disjoint_tenant_rate_cannot_take_b_verified_window() {
    const STARTS_PER_TENANT: usize = 4;

    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    member(&fixture, "tenant-b", "rate-b-only");
    let b = signed_admission(&fixture, "rate-b-only", 0).await;
    let mut readers = Vec::new();
    for index in 0..=STARTS_PER_TENANT {
        let subject = format!("rate-a-only-{index}");
        member(&fixture, "tenant-a", &subject);
        readers.push(signed_admission(&fixture, &subject, 0).await);
    }
    let mut registry = oidc.registry.lock().unwrap();
    let attributed_start = |reader: &Admitted, expected_tenant: &str| {
        let session = &registry.sessions[&reader.binding.session_id.to_string()];
        let mut keys = session.registry_tenants.as_ref().unwrap().keys();
        let tenant = keys.next().unwrap();
        assert!(
            keys.next().is_none(),
            "fixture must have one verified tenant"
        );
        assert_eq!(
            registry.tenant_limits.as_ref().unwrap().tenant_id(tenant),
            Some(expected_tenant)
        );
        (session.binding.principal.key().unwrap(), tenant)
    };
    let starts_a: Vec<_> = readers
        .iter()
        .map(|reader| attributed_start(reader, "tenant-a"))
        .collect();
    let (principal_b, tenant_b) = attributed_start(&b, "tenant-b");
    let ((excess_principal, tenant_a), starts_a) = starts_a.split_last().unwrap();

    // Use the same rate transition as native refresh, with one instant for
    // every start. The reversed-plan test above verifies HTTP ownership and
    // its recorded charge.
    registry.rate = None;
    let now = Instant::now();
    for (principal, tenant) in starts_a {
        registry.rate = Some(next_window(&registry, Some(principal), Some(*tenant), now).unwrap());
    }
    let Err((status, body)) = next_window(&registry, Some(excess_principal), Some(*tenant_a), now)
    else {
        panic!("the fifth start for tenant A must be refused");
    };
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body.0["error"]["code"], "capacity_exhausted");

    let admitted_b = next_window(&registry, Some(&principal_b), Some(tenant_b), now).unwrap();
    assert_eq!(admitted_b.started, now);
    assert_eq!(usize::from(admitted_b.tenants[tenant_a]), STARTS_PER_TENANT);
    assert_eq!(admitted_b.tenants[&tenant_b], 1);
}

#[tokio::test]
async fn orphan_native_intent_keeps_the_same_verified_tenant_until_audited_finish() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    member(&fixture, "tenant-a", "orphan-a-only");
    let a = signed_admission(&fixture, "orphan-a-only", 0).await;
    member(&fixture, "tenant-b", "orphan-b-only");
    let b = signed_admission(&fixture, "orphan-b-only", 0).await;
    reset_start_window(oidc);
    let (status, reply) = begin(&fixture, &a).await;
    assert_eq!(status, StatusCode::OK);
    let a_intent = Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap();
    let owner;
    let removed;
    {
        let mut registry = oidc.registry.lock().unwrap();
        owner = registry.native[&a_intent].registry_tenant.unwrap();
        assert_eq!(
            registry.tenant_limits.as_ref().unwrap().tenant_id(owner),
            Some("tenant-a")
        );
        // Fault cut: remove the real Session while retaining its exact intent,
        // durable pending receipt and original deadline, never fabricate one.
        removed = registry
            .sessions
            .remove(&a.binding.session_id.to_string())
            .unwrap();
        assert_eq!(
            removed.pending_refresh.as_ref().unwrap().registry_tenant,
            Some(owner)
        );
        let owners: Vec<_> = native_owners(&registry).collect();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].tenant, Some(owner));
        assert_eq!(*owners[0].binding, a.binding);
    }
    let (status, reply) = begin(&fixture, &b).await;
    assert_eq!(status, StatusCode::OK);
    let b_intent = Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        oidc.registry.lock().unwrap().native[&a_intent].registry_tenant,
        Some(owner)
    );
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 2);
    let before_refused_finish = oidc.audit.verify().unwrap();
    let (status, reply) = request(
        &fixture.state,
        json!({"kind":"finish","intent_id":a_intent,"outcome":"cancelled"}),
        Some(&a.raw),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply["error"]["code"], "audit_unavailable");
    assert_eq!(oidc.audit.verify().unwrap(), before_refused_finish);
    assert_eq!(
        oidc.registry.lock().unwrap().native[&a_intent].registry_tenant,
        Some(owner)
    );
    // The existing keeper refuses an orphan's terminal. Return the same live
    // Session object at this test cut, then exercise its normal audited finish.
    assert!(
        oidc.registry
            .lock()
            .unwrap()
            .sessions
            .insert(a.binding.session_id.to_string(), removed)
            .is_none()
    );
    finish(&fixture, &a, a_intent).await;
    finish(&fixture, &b, b_intent).await;
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    oidc.audit.verify().unwrap();
}

fn member(fixture: &Fixture, tenant_id: &str, subject: &str) {
    let principal = PrincipalRefV1::HumanOidc {
        issuer: fixture._issuer.issuer.clone(),
        subject: subject.into(),
    };
    for tenant in fixture.state.tenants.iter() {
        if tenant.tenant_id != tenant_id {
            assert!(
                tenant
                    .store
                    .sqlite()
                    .unwrap()
                    .human_account(&principal)
                    .unwrap()
                    .is_none()
            );
            continue;
        }
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic registry reader".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: tenant_id.into(),
                    organization_scope: format!("organization-{tenant_id}"),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }))
            .unwrap();
    }
}

async fn signed_admission(fixture: &Fixture, subject: &str, salt: usize) -> Admitted {
    // Fixture construction only: public nonce/admission routes still execute.
    // This is not a claim about how quickly anonymous starts can be produced.
    let oidc = fixture.state.oidc.as_ref().unwrap();
    reset_start_window(oidc);
    let (status, nonce) = request(&fixture.state, json!({"kind":"login"}), None).await;
    assert_eq!(status, StatusCode::OK, "setup: real login nonce required");
    let raw = fixture._issuer.token(
        "cli-public",
        subject,
        None,
        &[],
        &format!("registry-{subject}-{salt}"),
    );
    let verified = oidc.provider.verify_access(&raw).await.unwrap();
    assert_eq!(verified.subject(), subject);
    let response = gateway_router(fixture.state.clone())
        .oneshot(
            Request::post("/auth/oidc/admit")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::from(
                    json!({"version":1,"intent_id":nonce["intent_id"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), 35_000)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let id = Uuid::parse_str(value["authentication_id"].as_str().unwrap()).unwrap();
    let binding = oidc.registry.lock().unwrap().sessions[&id.to_string()]
        .binding
        .clone();
    assert_eq!(
        binding.principal,
        PrincipalRefV1::HumanOidc {
            issuer: fixture._issuer.issuer.clone(),
            subject: subject.into(),
        }
    );
    assert_eq!(
        oidc.authenticate(&raw).unwrap().authentication_id,
        id.to_string()
    );
    Admitted { raw, binding }
}

async fn begin(fixture: &Fixture, reader: &Admitted) -> (StatusCode, Value) {
    request(
        &fixture.state,
        json!({"kind":"refresh","authentication_id":reader.binding.session_id,
        "generation":reader.binding.generation}),
        Some(&reader.raw),
    )
    .await
}

async fn finish(fixture: &Fixture, reader: &Admitted, intent: Uuid) {
    let started = Instant::now();
    let (status, _) = request(
        &fixture.state,
        json!({"kind":"finish","intent_id":intent,"outcome":"cancelled"}),
        Some(&reader.raw),
    )
    .await;
    eprintln!(
        "D5_HARNESS phase=finish status={} duration_ms={}",
        status.as_u16(),
        started.elapsed().as_millis()
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "HARNESS_NONCONCLUSIVE: exact audited owner must retire"
    );
    assert!(
        !fixture
            .state
            .oidc
            .as_ref()
            .unwrap()
            .registry
            .lock()
            .unwrap()
            .native
            .contains_key(&intent)
    );
}

async fn anonymous_web_pressure(fixture: &Fixture) {
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let before = oidc.audit.verify().unwrap();
    for _ in 0..224 {
        // Only setup's anonymous rate window is advanced. The 224 transactions
        // are produced by status+start, never inserted into the private map.
        reset_start_window(oidc);
        let response = gateway_router(fixture.state.clone())
            .oneshot(
                Request::get("/auth/oidc/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("set-cookie").is_none());
        let response = gateway_router(fixture.state.clone())
            .oneshot(
                Request::post("/auth/oidc/start")
                    .header("content-type", "application/json")
                    .header("origin", "http://127.0.0.1:18100")
                    .header("sec-fetch-site", "same-origin")
                    .header("sec-fetch-mode", "cors")
                    .body(Body::from(json!({"version":1}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "real public start must reach discovery/PKCE"
        );
        assert!(response.headers().get("set-cookie").is_some());
        let bytes = axum::body::to_bytes(response.into_body(), 35_000)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            value["authorization_url"]
                .as_str()
                .unwrap()
                .contains("code_challenge=")
        );
        let correlation_token = value["correlation_token"].as_str().unwrap();
        assert_eq!(correlation_token.len(), 64);
    }
    assert_eq!(
        oidc.audit.verify().unwrap(),
        before,
        "anonymous setup must not spend durable capacity"
    );
    {
        let registry = oidc.registry.lock().unwrap();
        assert_eq!(registry.transactions.len(), 224);
        assert_eq!(anonymous_slots(&registry), 224);
        assert!(
            registry
                .transactions
                .values()
                .all(|entry| entry.pending.is_some()
                    && entry.created.elapsed()
                        < Duration::from_secs(oidc.config.provider.limits.pkce_ttl_seconds))
        );
    }
    assert_eq!(
        request(&fixture.state, json!({"kind":"login"}), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

fn assert_live_owners(
    oidc: &OidcGateway,
    readers: &[Admitted],
    pending: &[(usize, Uuid)],
    extra: usize,
    phase: &str,
) -> Value {
    let registry = oidc.registry.lock().unwrap();
    let now = oidc.admission_now();
    let owners = native_owners(&registry).count();
    let oldest = pending
        .iter()
        .filter_map(|(_, id)| registry.native.get(id))
        .map(|intent| now.saturating_duration_since(intent.created))
        .max()
        .unwrap_or_default();
    let remaining = pending
        .iter()
        .filter_map(|(index, _)| {
            registry
                .sessions
                .get(&readers[*index].binding.session_id.to_string())
                .and_then(|session| session.pending_refresh.as_ref())
                .map(|transition| transition.deadline.saturating_duration_since(now))
        })
        .min()
        .unwrap_or_default();
    eprintln!(
        "D5_HARNESS phase={phase} accepted_a={} live_native={owners} extra={extra} oldest_ms={} min_remaining_ms={}",
        pending.len(),
        oldest.as_millis(),
        remaining.as_millis()
    );
    assert_eq!(
        owners,
        pending.len() + extra,
        "HARNESS_NONCONCLUSIVE: accepted owners no longer all live at {phase}"
    );
    assert_eq!(anonymous_slots(&registry), 224);
    for (index, id) in pending {
        let owner = registry
            .native
            .get(id)
            .unwrap_or_else(|| panic!("HARNESS_NONCONCLUSIVE: accepted intent missing at {phase}"));
        let session = registry
            .sessions
            .get(&readers[*index].binding.session_id.to_string())
            .unwrap_or_else(|| {
                panic!("HARNESS_NONCONCLUSIVE: accepted session missing at {phase}")
            });
        let transition = session.pending_refresh.as_ref().unwrap();
        assert_eq!(owner.previous.as_ref(), Some(&readers[*index].binding));
        assert_eq!(owner.intention.as_ref(), Some(&transition.receipt));
        assert!(
            session.rotating
                && transition.deadline > now
                && now.saturating_duration_since(owner.created) < Duration::from_secs(15),
            "HARNESS_NONCONCLUSIVE: accepted owner expired at {phase}"
        );
        assert_eq!(transition.record.operation_id, *id);
    }
    json!({"phase":phase,"accepted_a":pending.len(),"live_native":owners,"extra_b":extra,
        "oldest_ms":oldest.as_millis(),"min_remaining_ms":remaining.as_millis(),
        "window_age_ms":registry.rate.as_ref().map(|window| now.saturating_duration_since(window.started).as_millis()),
        "window_anonymous":registry.rate.as_ref().map(|window| window.anonymous),
        "window_native":registry.rate.as_ref().map(|window| window.authenticated.values().map(|count|u32::from(*count)).sum::<u32>()),
        "window_total":registry.rate.as_ref().map(StartWindow::total)})
}

#[tokio::test]
async fn disjoint_tenant_native_owners_cannot_take_b_refresh_capacity() {
    let overall_started = Instant::now();
    let mut plan = capacity_plan(32, 720, 200);
    // Same fixture G1000/4MiB; redistribute finite parts to fund the baseline's
    //32 real owners plus their V3-compatible logout/refresh allowances. No G,
    // token lifetime, native deadline, semaphore or registry cap is increased.
    plan.tenants[0].capacity.max_serialized_bytes = 3328 * 1024;
    plan.tenants[1].capacity.max_serialized_bytes = 256 * 1024;
    let fixture = fixture_with_plan(1000, Some(plan)).await;
    eprintln!(
        "D5_HARNESS phase=fixture elapsed_ms={}",
        overall_started.elapsed().as_millis()
    );
    let sessions_started = Instant::now();
    let oidc = fixture.state.oidc.as_ref().unwrap();
    member(&fixture, "tenant-b", "registry-b");
    let positive = signed_admission(&fixture, "registry-b", 0).await;
    let after_anonymous = signed_admission(&fixture, "registry-b", 1).await;
    let target = signed_admission(&fixture, "registry-b", 2).await;
    let mut readers = Vec::new();
    for principal in 0..8 {
        let subject = format!("registry-a-{principal}");
        member(&fixture, "tenant-a", &subject);
        for session in 0..4 {
            readers.push(signed_admission(&fixture, &subject, session).await);
        }
    }
    assert_eq!(
        readers
            .iter()
            .map(|reader| reader.binding.session_id)
            .collect::<BTreeSet<_>>()
            .len(),
        32,
        "setup: pressure needs 32 distinct admitted sessions"
    );
    eprintln!(
        "D5_HARNESS phase=signed_sessions duration_ms={}",
        sessions_started.elapsed().as_millis()
    );
    reset_start_window(oidc);
    let (status, reply) = begin(&fixture, &positive).await;
    assert_eq!(status, StatusCode::OK, "positive B before pressure");
    finish(
        &fixture,
        &positive,
        Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap(),
    )
    .await;
    let anonymous_started = Instant::now();
    anonymous_web_pressure(&fixture).await;
    eprintln!(
        "D5_HARNESS phase=anonymous224 duration_ms={}",
        anonymous_started.elapsed().as_millis()
    );
    let (status, reply) = begin(&fixture, &after_anonymous).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "published anonymous/native separation must remain effective"
    );
    finish(
        &fixture,
        &after_anonymous,
        Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap(),
    )
    .await;

    freeze_admission_clock(oidc);
    let pressure_started = Instant::now();
    let mut pending = Vec::new();
    let mut responses = 0;
    let mut wave_observations = Vec::new();
    for wave in 0..4 {
        // Cross the real rate boundary between waves, including the positive
        // B probe's window. Previously issued owners keep their original deadlines.
        advance_admission_clock(oidc, Duration::from_secs(1));
        assert_eq!(
            (0..8)
                .map(|principal| readers[principal * 4 + wave]
                    .binding
                    .principal
                    .key()
                    .unwrap())
                .collect::<BTreeSet<_>>()
                .len(),
            8,
            "setup: each wave needs eight distinct verified principals"
        );
        let wave_started = Instant::now();
        let fixture_ref = &fixture;
        let readers_ref = &readers;
        let results = futures_util::future::join_all((0..8).map(|principal| async move {
            // Each wave uses eight distinct principals. Each principal's four
            // sessions are distinct across waves, below its original 4/2 caps.
            let index = principal * 4 + wave;
            let request_started = Instant::now();
            let (status, reply) = begin(fixture_ref, &readers_ref[index]).await;
            (index, status, reply, request_started.elapsed())
        }))
        .await;
        assert_eq!(results.len(), 8);
        let mut refused = 0;
        for (index, status, reply, duration) in results {
            responses += 1;
            eprintln!(
                "D5_HARNESS phase=native_response wave={} principal={} status={} duration_ms={}",
                wave + 1,
                index / 4,
                status.as_u16(),
                duration.as_millis()
            );
            if status == StatusCode::TOO_MANY_REQUESTS {
                assert_eq!(reply["error"]["code"], "capacity_exhausted");
                refused += 1;
                continue;
            }
            assert_eq!(
                status,
                StatusCode::OK,
                "HARNESS_NONCONCLUSIVE: pressure failed outside volatile capacity"
            );
            pending.push((
                index,
                Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap(),
            ));
        }
        eprintln!(
            "D5_HARNESS phase=native_wave wave={} responses=8 refused={refused} duration_ms={} pressure_ms={}",
            wave + 1,
            wave_started.elapsed().as_millis(),
            pressure_started.elapsed().as_millis()
        );
        // On FIX, four starts per tenant/window can refuse half a wave. All
        // 32 offers still run; cumulative successes must remain genuine owners.
        wave_observations.push(assert_live_owners(
            oidc,
            &readers,
            &pending,
            0,
            "after_wave",
        ));
    }
    assert_eq!(responses, 32);
    assert!(
        matches!(pending.len(), 16 | 32),
        "HARNESS_NONCONCLUSIVE: intended tenant share or actual baseline global cap required"
    );
    let audit_started = Instant::now();
    let before_b = oidc.audit.verify().unwrap();
    assert!(before_b.records < 1000);
    assert!(before_b.serialized_bytes < fixture.audit_policy.max_serialized_bytes);
    eprintln!(
        "D5_HARNESS phase=pre_b_audit duration_ms={}",
        audit_started.elapsed().as_millis()
    );
    let authentication_started = Instant::now();
    oidc.authenticate(&target.raw).unwrap();
    eprintln!(
        "D5_HARNESS phase=pre_b_authenticate duration_ms={}",
        authentication_started.elapsed().as_millis()
    );
    let owners_before_b = assert_live_owners(oidc, &readers, &pending, 0, "before_b");
    let b_started = Instant::now();
    let (b_status, b_reply) = begin(&fixture, &target).await;
    let b_duration = b_started.elapsed();
    eprintln!(
        "D5_HARNESS phase=b_probe status={} duration_ms={} pressure_ms={}",
        b_status.as_u16(),
        b_duration.as_millis(),
        pressure_started.elapsed().as_millis()
    );
    let owners_after_b = assert_live_owners(
        oidc,
        &readers,
        &pending,
        usize::from(b_status == StatusCode::OK),
        "after_b",
    );
    let after_b = oidc.audit.verify().unwrap();
    if b_status != StatusCode::OK {
        assert_eq!(
            b_status,
            StatusCode::TOO_MANY_REQUESTS,
            "only the native capacity oracle may refuse B"
        );
        assert_eq!(b_reply["error"]["code"], "capacity_exhausted");
        assert_eq!(
            after_b, before_b,
            "refusal cannot consume a durable intention"
        );
    }
    // This is an observed pre-cleanup fact bundle, not the final RED verdict.
    // It is emitted only after every live/receipt/status/audit precondition
    // above. No token, correlation cookie, principal or provider body is logged.
    eprintln!(
        "D5_ORACLE_OBSERVATION {}",
        json!({"version":1,"stage":"before_cleanup_not_final_red",
        "accepted_a":pending.len(),"b_status":b_status.as_u16(),"b_error_code":b_reply["error"]["code"].as_str(),
        "baseline_slots_observed":pending.len()==32 && b_status==StatusCode::TOO_MANY_REQUESTS,
        "responses":responses,"waves":wave_observations,"owners_before_b":owners_before_b,"owners_after_b":owners_after_b,
        "audit_before":{"records":before_b.records,"bytes":before_b.serialized_bytes,"sequence":before_b.head.sequence,"head_hash":before_b.head.hash},
        "audit_after":{"records":after_b.records,"bytes":after_b.serialized_bytes,"sequence":after_b.head.sequence,"head_hash":after_b.head.hash},
        "b_duration_ms":b_duration.as_millis(),"pressure_ms":pressure_started.elapsed().as_millis(),"total_ms":overall_started.elapsed().as_millis()})
    );
    // Cleanup precedes the final RED/FIX assertion; preserve the genuine
    // baseline refusal while checking exact owner retirement/reopen too.
    let cleanup_started = Instant::now();
    if b_status == StatusCode::OK {
        finish(
            &fixture,
            &target,
            Uuid::parse_str(b_reply["intent_id"].as_str().unwrap()).unwrap(),
        )
        .await;
    }
    for (index, intent) in &pending {
        finish(&fixture, &readers[*index], *intent).await;
    }
    eprintln!(
        "D5_HARNESS phase=cleanup duration_ms={}",
        cleanup_started.elapsed().as_millis()
    );
    assert_eq!(native_owners(&oidc.registry.lock().unwrap()).count(), 0);
    let verified = oidc.audit.verify().unwrap();
    let reopened = AuthenticationAudit::open_existing(
        &fixture._files.path().join("authentication.sqlite"),
        fixture.installation_id,
        fixture.audit_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(reopened.verify().unwrap(), verified);
    eprintln!(
        "D5_HARNESS phase=verified_reopen total_ms={}",
        overall_started.elapsed().as_millis()
    );
    if b_status != StatusCode::OK {
        assert_eq!(
            pending.len(),
            32,
            "HARNESS_NONCONCLUSIVE: the slots RED requires exactly 32 live A owners at B"
        );
    }
    assert_eq!(
        b_status,
        StatusCode::OK,
        "D5_TENANT_NATIVE_CAPACITY: live native owners from disjoint A denied verified B; A={}",
        pending.len()
    );
    assert_eq!(
        pending.len(),
        16,
        "A cannot borrow B's16 native owner slots"
    );
}
