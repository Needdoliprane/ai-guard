//! Real router/SQLite first; faults change only its already committed response.
//! A receipt is audit data, never an original owner or an emission permission.
use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Store},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    middleware::Next,
    response::IntoResponse,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const RUNNER: &str = "resume-exact-synthetic-runner";
fn event() -> Event {
    let mut event = Event::new(crate::audit::RETIREMENT_RESUME_EVENT_KIND);
    event.source = "ai-guard-broker".into();
    event.coverage = "native".into();
    event.data = json!({"version":1,"operation_id":Uuid::new_v4(),"command_id":event.id,
        "retirement_id":Uuid::new_v4(),"arm_id":Uuid::new_v4(),"original_epoch":Uuid::new_v4(),"generation":0,
        "identity_sha256":"a".repeat(64),"evidence_sha256":"b".repeat(64),"pending_ack_sha256":"c".repeat(64),
        "deadline_ns_decimal":"123456789"});
    event
}

#[tokio::test]
async fn retirement_resume_http_requires_the_entire_actual_stored_event() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Arc::new(
        Store::open_for_tenant(
            &directory.path().join("audit.sqlite"),
            "resume-exact",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    store
        .migrate_control_v3(&directory.path().join("backup3"))
        .unwrap();
    store
        .migrate_identity_v4(&directory.path().join("backup4"))
        .unwrap();
    store
        .migrate_human_access_v5(&directory.path().join("backup5"))
        .unwrap();
    let mode = Arc::new(AtomicUsize::new(0));
    let selected = Arc::clone(&mode);
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&requests);
    let app = gateway_router(GatewayState::new(
        Arc::clone(&store),
        RUNNER.into(),
        "synthetic-viewer".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
    .layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let mode = Arc::clone(&selected);
            let counted = Arc::clone(&counted);
            async move {
                if request.uri().path() != "/control/api/v1/control/events" {
                    return next.run(request).await;
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let response = next.run(request).await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "real runner/Store route must commit before response fault"
                );
                let (_, body) = response.into_parts();
                let mut value: Value =
                    serde_json::from_slice(&to_bytes(body, 512 * 1024).await.unwrap()).unwrap();
                match mode.load(Ordering::SeqCst) {
                    1 => value = json!({"seq":value["seq"]}),
                    2 => value["id"] = json!(Uuid::new_v4()),
                    3 => value["data"]["pending_ack_sha256"] = json!("d".repeat(64)),
                    _ => (),
                }
                axum::Json(value).into_response()
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, wait) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = wait.await;
            })
            .await
            .unwrap();
    });
    let outcome: Result<(), ColdAuditHttpError> = async {
        let client = AuditClient::new(&origin, RUNNER).unwrap();
        let exact = event();
        let receipt = client
            .post_retirement_resume_event(&exact, Instant::now() + Duration::from_secs(10))
            .await?;
        assert!(receipt.seq() > 0);
        assert_eq!(
            serde_json::to_value(&receipt.stored_event().event).unwrap(),
            serde_json::to_value(&exact).unwrap()
        );
        for fault in 1..=3 {
            mode.store(fault, Ordering::SeqCst);
            let request = event();
            assert_eq!(
                client
                    .post_retirement_resume_event(
                        &request,
                        Instant::now() + Duration::from_secs(10)
                    )
                    .await
                    .unwrap_err(),
                ColdAuditHttpError::InvalidResponse
            );
            assert!(
                store
                    .events(&EventQuery {
                        kind: Some(crate::audit::RETIREMENT_RESUME_EVENT_KIND.into()),
                        limit: Some(10),
                        ..Default::default()
                    })
                    .unwrap()
                    .iter()
                    .any(|row| row.event.id == request.id),
                "fault followed a real durable commit"
            );
        }
        let count = requests.load(Ordering::SeqCst);
        let head = store.integrity().unwrap().head_hash;
        assert_eq!(
            client
                .post_retirement_resume_event(&event(), Instant::now())
                .await
                .unwrap_err(),
            ColdAuditHttpError::Expired
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            count,
            "expired deadline must cause no HTTP send"
        );
        assert_eq!(store.integrity().unwrap().head_hash, head);
        // A collision with the retained binding must fail before transport;
        // masking is never disabled to make a receipt compare successfully.
        let masked = client.with_secrets(vec!["a".repeat(64)]);
        assert_eq!(
            masked
                .post_retirement_resume_event(&event(), Instant::now() + Duration::from_secs(10))
                .await
                .unwrap_err(),
            ColdAuditHttpError::InvalidRequest
        );
        assert_eq!(requests.load(Ordering::SeqCst), count);
        Ok(())
    }
    .await;
    let _ = stop.send(());
    task.await.unwrap();
    outcome.unwrap();
}
