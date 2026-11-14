use super::*;
use crate::control::egress_policy::{
    EgressPolicyExpectedScopeV1, EgressPolicyObservedCeilingV1, EgressPolicyObservedHeadV1,
    EgressPolicyObservedSessionResourceV1, EgressPolicySelectorV1, REQUEST_BYTES,
};
#[cfg(unix)]
use crate::{
    api::{GatewayState, gateway_router},
    audit::Store,
    policy::Policy,
    redaction::Redactor,
};
use futures_util::FutureExt;
use std::time::Duration;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_sdk_uses_actual_reserved_owned15_metadata_and_closed_role_errors() {
    let mut fixture = Store::owned_control_fixture_for_test(15);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let namespace = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .durable_namespace_id
        .to_string();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let retirement = state.human_retirement().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut task = tokio::spawn(async move {
        axum::serve(listener, gateway_router(state))
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let client = AuditClient::new(&origin, "synthetic-A-runner").unwrap();
    let exercised = std::panic::AssertUnwindSafe(async {
        assert_eq!(
            client
                .control_supervision_identity()
                .await
                .unwrap()
                .schema_version,
            15
        );
        let request = EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant).unwrap();
        let observation = client.egress_inspect_policy(&request).await.unwrap();
        assert_eq!(observation.scope().tenant_id(), "A");
        assert_eq!(observation.scope().control_namespace_uuid(), namespace);
        assert_eq!(*observation.head(), EgressPolicyObservedHeadV1::Absent);
        assert_eq!(
            observation.session_resource(),
            EgressPolicyObservedSessionResourceV1::NotApplicable
        );
        assert!(observation.historical());
        assert_eq!(observation.coverage(), "policy_registry_metadata");
        assert_eq!(
            EgressPolicyInspectionV1::from_canonical_bytes(&observation.canonical_bytes().unwrap())
                .unwrap(),
            observation
        );
        let denied = AuditClient::new(&origin, "synthetic-A-viewer").unwrap();
        assert_eq!(
            denied.egress_inspect_policy(&request).await.unwrap_err(),
            E::Forbidden
        );
    })
    .catch_unwind()
    .await;
    drop(client);
    let _ = stop.send(());
    let orderly = match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
        Ok(result) => result.is_ok_and(|result| result.is_ok()),
        Err(_) => {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(30), &mut task).await;
            false
        }
    };
    retirement.retire();
    tokio::time::timeout(Duration::from_secs(30), retirement.drain())
        .await
        .unwrap()
        .unwrap();
    assert!(
        orderly,
        "inspection SDK fixture server did not drain normally"
    );
    if let Err(panic) = exercised {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_sdk_rejects_invalid_wire_once_without_fallback_or_remote_text() {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    let request = EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant).unwrap();
    let scope =
        EgressPolicyExpectedScopeV1::new("A", "00000000-0000-0000-0000-000000000048").unwrap();
    let good = EgressPolicyInspectionV1::observed(
        scope.clone(),
        EgressPolicySelectorV1::Tenant,
        EgressPolicyObservedHeadV1::Absent,
        EgressPolicyObservedCeilingV1::NotApplicable,
        EgressPolicyObservedSessionResourceV1::NotApplicable,
        "2026-11-14T00:00:00.000Z".into(),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    let wrong_selector = EgressPolicyInspectionV1::observed(
        scope,
        EgressPolicySelectorV1::session("00000000-0000-0000-0000-000000000049").unwrap(),
        EgressPolicyObservedHeadV1::Absent,
        EgressPolicyObservedCeilingV1::Absent,
        EgressPolicyObservedSessionResourceV1::Absent,
        "2026-11-14T00:00:00.000Z".into(),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    let reflected_token = EgressPolicyInspectionV1::observed(
        EgressPolicyExpectedScopeV1::new(
            "synthetic-inspection-secret",
            "00000000-0000-0000-0000-000000000048",
        )
        .unwrap(),
        EgressPolicySelectorV1::Tenant,
        EgressPolicyObservedHeadV1::Absent,
        EgressPolicyObservedCeilingV1::NotApplicable,
        EgressPolicyObservedSessionResourceV1::NotApplicable,
        "2026-11-14T00:00:00.000Z".into(),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    let mut noncanonical = good.clone();
    noncanonical.push(b' ');
    let replies = VecDeque::from([
        (200, "application/json", "no-store", noncanonical),
        (
            200,
            "application/json",
            "no-store",
            vec![b' '; RECEIPT_BYTES + 1],
        ),
        (200, "application/json", "no-store", wrong_selector),
        (200, "application/json", "no-store", reflected_token),
        (200, "text/plain", "no-store", good.clone()),
        (200, "application/json", "public", good),
        (
            404,
            "application/json",
            "no-store",
            b"private remote failure".to_vec(),
        ),
        (
            409,
            "application/json",
            "no-store",
            br#"{"error":{"code":"unknown","message":"private remote failure"}}"#.to_vec(),
        ),
        (
            503,
            "application/json",
            "no-store",
            br#"{"error":{"code":"egress_policy_busy","message":"private remote failure"}}"#
                .to_vec(),
        ),
    ]);
    let replies = Arc::new(Mutex::new(replies));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (handler_replies, handler_seen) = (replies.clone(), seen.clone());
    // This endpoint exercises only the untrusted SDK response boundary. The
    // owned-source test above is the separate positive authority witness.
    let router =
        axum::Router::new().fallback(move |incoming: axum::http::Request<axum::body::Body>| {
            let (replies, seen) = (handler_replies.clone(), handler_seen.clone());
            async move {
                let (parts, body) = incoming.into_parts();
                let bytes = axum::body::to_bytes(body, REQUEST_BYTES).await.unwrap();
                let authorization = parts
                    .headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                seen.lock().unwrap().push((
                    parts.method,
                    parts.uri.to_string(),
                    authorization,
                    bytes.to_vec(),
                ));
                let (status, content_type, cache_control, body) =
                    replies.lock().unwrap().pop_front().unwrap_or((
                        500,
                        "text/plain",
                        "no-store",
                        b"unexpected extra request".to_vec(),
                    ));
                axum::http::Response::builder()
                    .status(status)
                    .header(axum::http::header::CONTENT_TYPE, content_type)
                    .header(axum::http::header::CACHE_CONTROL, cache_control)
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let client = AuditClient::new(&origin, "synthetic-inspection-secret").unwrap();
    let exercised = std::panic::AssertUnwindSafe(async {
        let expected = [
            E::InvalidResponse,
            E::InvalidResponse,
            E::InvalidResponse,
            E::InvalidResponse,
            E::InvalidResponse,
            E::InvalidResponse,
            E::Unavailable,
            E::InvalidResponse,
            E::Busy,
        ];
        for (index, failure) in expected.into_iter().enumerate() {
            assert_eq!(
                client.egress_inspect_policy(&request).await.unwrap_err(),
                failure
            );
            assert_eq!(seen.lock().unwrap().len(), index + 1);
        }
        assert!(replies.lock().unwrap().is_empty());
        for (method, path, authorization, body) in seen.lock().unwrap().iter() {
            assert_eq!(*method, reqwest::Method::POST);
            assert_eq!(path, "/control/api/v1/egress/policies/inspect");
            assert_eq!(authorization, "Bearer synthetic-inspection-secret");
            assert_eq!(body, &request.canonical_bytes().unwrap());
        }
    })
    .catch_unwind()
    .await;
    drop(client);
    let _ = stop.send(());
    let orderly = match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
        Ok(result) => result.is_ok_and(|result| result.is_ok()),
        Err(_) => {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(30), &mut task).await;
            false
        }
    };
    assert!(
        orderly,
        "inspection SDK transport fixture did not drain normally"
    );
    if let Err(panic) = exercised {
        std::panic::resume_unwind(panic);
    }
}
