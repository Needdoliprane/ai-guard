use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::Store,
    control::egress_policy::{
        EgressPolicyObservedHeadV1, EgressPolicyObservedSessionResourceV1,
        EgressPolicySelectorV1,
    },
    policy::Policy,
    redaction::Redactor,
};
use futures_util::FutureExt;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_sdk_uses_actual_reserved_owned15_metadata_and_closed_role_errors() {
    let mut fixture = Store::owned_control_fixture_for_test(15);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let namespace = bundle.owner.closure().sources.iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap().durable_namespace_id.to_string();
    let state = GatewayState::for_owned_human_sources(
        bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![]),
    ).unwrap();
    let retirement = state.human_retirement().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut task = tokio::spawn(async move {
        axum::serve(listener, gateway_router(state)).with_graceful_shutdown(async {
            let _ = stopped.await;
        }).await
    });
    let client = AuditClient::new(&origin, "synthetic-A-runner").unwrap();
    let exercised = std::panic::AssertUnwindSafe(async {
        assert_eq!(client.control_supervision_identity().await.unwrap().schema_version, 15);
        let request = EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant).unwrap();
        let observation = client.egress_inspect_policy(&request).await.unwrap();
        assert_eq!(observation.scope().tenant_id(), "A");
        assert_eq!(observation.scope().control_namespace_uuid(), namespace);
        assert_eq!(*observation.head(), EgressPolicyObservedHeadV1::Absent);
        assert_eq!(observation.session_resource(), EgressPolicyObservedSessionResourceV1::NotApplicable);
        assert!(observation.historical());
        assert_eq!(observation.coverage(), "policy_registry_metadata");
        assert_eq!(EgressPolicyInspectionV1::from_canonical_bytes(&observation.canonical_bytes().unwrap()).unwrap(), observation);
        let denied = AuditClient::new(&origin, "synthetic-A-viewer").unwrap();
        assert_eq!(denied.egress_inspect_policy(&request).await.unwrap_err(), E::Forbidden);
    }).catch_unwind().await;
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
    tokio::time::timeout(Duration::from_secs(30), retirement.drain()).await.unwrap().unwrap();
    assert!(orderly, "inspection SDK fixture server did not drain normally");
    if let Err(panic) = exercised {
        std::panic::resume_unwind(panic);
    }
}
