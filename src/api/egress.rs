//! Broker-facing egress routes and the tenant read of egress facts.
//!
//! The tenant is the one of the authenticated runner (or reader) credential;
//! no body field selects it. Decisions are durable in the tenant journal before
//! the response, and an audit failure answers 503 so the broker refuses the
//! connection. Free-text causes never leave the gateway.

use super::*;
use crate::{
    audit::egress_decisions::EgressJournalError,
    control::egress_decision::{
        EgressChannelOpeningReceiptV1, EgressChannelOpeningRequestV1, EgressConnectPermitReceiptV1,
        EgressConnectPermitRequestV1, EgressDecideRequestV1, EgressDecisionV1,
        EgressObserveRequestV1, EgressReceiptV1, EgressRenewRequestV1, EgressRenewResponseV1,
        deserialize_object,
    },
};

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/egress/channel-openings", post(open_channel))
        .route("/egress/connect-permits", post(issue_connect_permit))
        .route("/egress/decisions", post(decide))
        .route("/egress/observations", post(observe))
        .route("/egress/renewals", post(renew))
        .route("/egress/events", get(events))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn invalid_request(status: StatusCode) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"error":{"code":"egress_invalid","retry_action":"none"}})),
    )
}

fn journal_failure(failure: anyhow::Error) -> (StatusCode, Json<Value>) {
    match failure.downcast_ref::<EgressJournalError>() {
        Some(EgressJournalError::Conflict) => (
            StatusCode::CONFLICT,
            Json(json!({"error":{"code":"egress_conflict","retry_action":"none"}})),
        ),
        Some(EgressJournalError::Invalid) => invalid_request(StatusCode::BAD_REQUEST),
        None => unavailable(failure),
    }
}

fn healthy(tenant: &TenantState) -> ApiResult<()> {
    if tenant.store.healthy() {
        Ok(())
    } else {
        Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Intégrité de l'audit défaillante : sortie réseau refusée.",
        ))
    }
}

async fn decide(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressDecideRequestV1>,
) -> ApiResult<Json<EgressDecisionV1>> {
    tenant.write_quota()?;
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            let store = tenant.store.sqlite_api()?;
            let policy = store.egress_policy();
            store
                .egress_decide(&request, &policy, now_ms())
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

async fn observe(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressObserveRequestV1>,
) -> ApiResult<Json<EgressReceiptV1>> {
    write_egress(tenant, move |store| store.egress_observe(&request)).await
}

#[derive(Deserialize)]
#[serde(transparent)]
struct OpeningRequestJson(
    #[serde(deserialize_with = "deserialize_object")] EgressChannelOpeningRequestV1,
);

async fn open_channel(
    RunnerIdentity(tenant): RunnerIdentity,
    request: Result<Json<OpeningRequestJson>, JsonRejection>,
) -> ApiResult<Json<EgressChannelOpeningReceiptV1>> {
    let Json(OpeningRequestJson(request)) =
        request.map_err(|rejection| invalid_request(rejection.status()))?;
    write_egress(tenant, move |store| store.egress_open_channel(&request)).await
}

#[derive(Deserialize)]
#[serde(transparent)]
struct ConnectPermitRequestJson(
    #[serde(deserialize_with = "deserialize_object")] EgressConnectPermitRequestV1,
);

async fn issue_connect_permit(
    RunnerIdentity(tenant): RunnerIdentity,
    request: Result<Json<ConnectPermitRequestJson>, JsonRejection>,
) -> ApiResult<Json<EgressConnectPermitReceiptV1>> {
    let Json(ConnectPermitRequestJson(request)) =
        request.map_err(|rejection| invalid_request(rejection.status()))?;
    write_egress(tenant, move |store| {
        store.egress_issue_connect_permit(&request)
    })
    .await
}

async fn write_egress<T: Send + 'static>(
    tenant: TenantState,
    record: impl FnOnce(&Store) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<Json<T>> {
    tenant.write_quota()?;
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            record(tenant.store.sqlite_api()?.as_ref())
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

async fn renew(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressRenewRequestV1>,
) -> ApiResult<Json<EgressRenewResponseV1>> {
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            let store = tenant.store.sqlite_api()?;
            let policy = store.egress_policy();
            store
                .egress_renew(&request, &policy, now_ms())
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EgressEventsQuery {
    session_id: String,
    limit: Option<usize>,
}

async fn events(
    ReadIdentity(tenant, _): ReadIdentity,
    Query(query): Query<EgressEventsQuery>,
) -> ApiResult<Json<Value>> {
    uuid::Uuid::parse_str(&query.session_id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Session invalide."))?;
    tenant
        .blocking(true, move |tenant| {
            let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
            let items = tenant
                .store
                .sqlite_api()?
                .egress_session_events(&query.session_id, limit)
                .map_err(unavailable)?;
            let complete = items.len() < limit;
            Ok(Json(json!({
                "items": items,
                "complete": complete,
                "coverage": {
                    "proxy_requests": "observed_by_broker",
                    "direct_attempts": "direct_attempts_unobserved",
                    "tls": "opaque",
                    "remote_tools": "remote_unmediated"
                }
            })))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::egress_decisions::tests::loopback_store_fixture;
    use crate::control::egress_decision::EgressChannelOpeningDispositionV1;
    use crate::egress::ids::IdV1;
    use tower::ServiceExt;

    #[tokio::test]
    async fn connect_permits_authenticate_before_strict_json_and_hide_rejection_details() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant-a.sqlite"),
                "tenant-a",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let router = gateway_router(GatewayState::new(
            store.clone(),
            "permit-runner".into(),
            "permit-viewer".into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        ));
        let post = |token, body| {
            router.clone().oneshot(
                axum::http::Request::post("/api/v1/egress/connect-permits")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
            )
        };
        let before = store.head().unwrap();
        for (token, expected) in [
            ("unknown-permit-token", StatusCode::UNAUTHORIZED),
            ("permit-viewer", StatusCode::FORBIDDEN),
        ] {
            let pending_body = Body::from_stream(futures_util::stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >());
            let response =
                tokio::time::timeout(std::time::Duration::from_secs(5), post(token, pending_body))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(response.status(), expected);
        }
        let private_field = "private_fixture_permit_field";
        for (request, expected) in [
            ("{".to_owned(), StatusCode::BAD_REQUEST),
            ("[]".to_owned(), StatusCode::UNPROCESSABLE_ENTITY),
            (
                format!(r#"{{"{private_field}":true}}"#),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let response = post("permit-runner", Body::from(request)).await.unwrap();
            assert_eq!(response.status(), expected);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(!text.contains(private_field));
            assert_eq!(
                serde_json::from_str::<Value>(text).unwrap(),
                json!({"error":{"code":"egress_invalid","retry_action":"none"}})
            );
        }
        assert_eq!(store.head().unwrap(), before);
    }

    #[tokio::test]
    async fn connect_permits_return_the_exact_committed_receipt_for_the_runner_tenant() {
        use crate::control::egress_decision::{
            EgressConnectPermitDispositionV1, egress_resolution_digest_v1,
        };

        const AUTHORITY: &str = "fixture.egress.test:8443";
        const OPENINGS: &str = "/api/v1/egress/channel-openings";
        const DECISIONS: &str = "/api/v1/egress/decisions";
        const PERMITS: &str = "/api/v1/egress/connect-permits";
        const RUNNER_A: &str = "permit-runner-a";
        const RUNNER_B: &str = "permit-runner-b";

        async fn receipt<T: serde::de::DeserializeOwned>(response: Response) -> T {
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            serde_json::from_slice(&bytes).unwrap()
        }

        let (directory, a, binding) = loopback_store_fixture(AUTHORITY);
        let b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant-b.sqlite"),
                "tenant-b",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let router = gateway_router(
            GatewayState::for_tenants(
                vec![
                    TenantGateway {
                        tenant_id: "tenant-a".into(),
                        store: a.clone(),
                        runner_token: RUNNER_A.into(),
                        viewer_token: "permit-viewer-a".into(),
                    },
                    TenantGateway {
                        tenant_id: "tenant-b".into(),
                        store: b.clone(),
                        runner_token: RUNNER_B.into(),
                        viewer_token: "permit-viewer-b".into(),
                    },
                ],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        let post = |path, token, body: Value| {
            router.clone().oneshot(
                axum::http::Request::post(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
        };
        let opening = EgressChannelOpeningRequestV1 {
            version: 1,
            observation_id: IdV1::random(),
            binding: binding.clone(),
        };
        let opened: EgressChannelOpeningReceiptV1 = receipt(
            post(OPENINGS, RUNNER_A, serde_json::to_value(&opening).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(opened.disposition, EgressChannelOpeningDispositionV1::Fresh);
        let decision = EgressDecideRequestV1 {
            version: 1,
            decision_id: IdV1::random(),
            binding: binding.clone(),
            flow_id: 1,
            authority: AUTHORITY.into(),
        };
        let allowed: EgressDecisionV1 = receipt(
            post(
                DECISIONS,
                RUNNER_A,
                serde_json::to_value(&decision).unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(allowed.allows(&decision));
        assert!(opened.receipt.audit_seq < allowed.receipt.audit_seq);

        // Only the request reaches SQLite. The test does not start a resolver
        // or open a socket to the address in this fixture.
        let ip = "93.184.216.34".parse().unwrap();
        let request = EgressConnectPermitRequestV1 {
            version: 1,
            opening_id: opening.observation_id,
            decision_id: decision.decision_id,
            binding,
            flow_id: decision.flow_id,
            ip,
            resolution_digest: egress_resolution_digest_v1(&[ip]).unwrap(),
        };
        let body = serde_json::to_value(&request).unwrap();
        let heads = (a.head().unwrap(), b.head().unwrap());
        assert_eq!(
            post(PERMITS, RUNNER_B, body.clone())
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!((a.head().unwrap(), b.head().unwrap()), heads);

        let fresh: EgressConnectPermitReceiptV1 =
            receipt(post(PERMITS, RUNNER_A, body.clone()).await.unwrap()).await;
        assert!(fresh.is_fresh_for(&request, 8443));
        assert_eq!(fresh.receipt.audit_seq, heads.0 + 1);
        assert_eq!(a.head().unwrap(), fresh.receipt.audit_seq);
        let replay: EgressConnectPermitReceiptV1 =
            receipt(post(PERMITS, RUNNER_A, body).await.unwrap()).await;
        assert_eq!(
            replay,
            EgressConnectPermitReceiptV1 {
                disposition: EgressConnectPermitDispositionV1::Replayed,
                ..fresh
            }
        );

        let ip = "93.184.216.35".parse().unwrap();
        let different = EgressConnectPermitRequestV1 {
            ip,
            resolution_digest: egress_resolution_digest_v1(&[ip]).unwrap(),
            ..request
        };
        assert_eq!(
            post(PERMITS, RUNNER_A, serde_json::to_value(&different).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            (a.head().unwrap(), b.head().unwrap()),
            (heads.0 + 1, heads.1)
        );
    }

    #[tokio::test]
    async fn channel_openings_require_a_runner_and_use_its_tenant() {
        let (directory, a, binding) = loopback_store_fixture("fixture.egress.test:8443");
        let b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant-b.sqlite"),
                "tenant-b",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: "opening-runner-a".into(),
                    viewer_token: "opening-viewer-a".into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: "opening-runner-b".into(),
                    viewer_token: "opening-viewer-b".into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let router = gateway_router(state);
        let post = |token, body: Value| {
            router.clone().oneshot(
                axum::http::Request::post("/api/v1/egress/channel-openings")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
        };
        let request = EgressChannelOpeningRequestV1 {
            version: 1,
            observation_id: IdV1::random(),
            binding,
        };
        let body = serde_json::to_value(&request).unwrap();
        let before = (a.head().unwrap(), b.head().unwrap());
        assert_eq!(
            post("unknown-opening-token", json!({}))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post("opening-viewer-a", json!({})).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post("opening-runner-b", body.clone())
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        let mut invalid = body.clone();
        invalid["version"] = json!(2);
        assert_eq!(
            post("opening-runner-a", invalid).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        let mut invalid = body.clone();
        invalid["observation"] = json!({"kind":"channel_closed"});
        assert_eq!(
            post("opening-runner-a", invalid).await.unwrap().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            post(
                "opening-runner-a",
                json!([request.version, request.observation_id, request.binding]),
            )
            .await
            .unwrap()
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let private_field = "private_fixture_opening_field";
        let mut invalid = body.clone();
        invalid[private_field] = json!(true);
        let response = post("opening-runner-a", invalid).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains(private_field));
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"error":{"code":"egress_invalid","retry_action":"none"}})
        );
        assert_eq!((a.head().unwrap(), b.head().unwrap()), before);

        let response = post("opening-runner-a", body.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let opened: EgressChannelOpeningReceiptV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(opened.version, 1);
        assert_eq!(opened.observation_id, request.observation_id);
        assert_eq!(opened.binding, request.binding);
        assert_eq!(opened.disposition, EgressChannelOpeningDispositionV1::Fresh);
        assert_eq!(opened.receipt.audit_seq, before.0 + 1);

        let response = post("opening-runner-a", body).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let replayed: EgressChannelOpeningReceiptV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            replayed,
            EgressChannelOpeningReceiptV1 {
                disposition: EgressChannelOpeningDispositionV1::Replayed,
                ..opened
            }
        );
        assert_eq!(
            (a.head().unwrap(), b.head().unwrap()),
            (before.0 + 1, before.1)
        );
    }
}
