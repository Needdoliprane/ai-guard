use super::*;
use crate::{api::{GatewayState,gateway_router}, audit::Store, control::egress_policy::*, policy::Policy, redaction::Redactor};
use serde_json::json;
use std::time::Duration;
use futures_util::FutureExt;

#[test]
fn closed_error_codes_never_expose_remote_text_or_turn_refusal_into_success() {
    assert_eq!(policy_error(409,br#"{"error":{"code":"egress_policy_scope_conflict","message":"private"}} "#),E::ScopeConflict);
    assert_eq!(policy_error(409,b"arbitrary private text"),E::InvalidResponse);
    assert_eq!(policy_error(503,b"arbitrary private text"),E::Unavailable);
    assert_eq!(policy_error(401,b"ignored"),E::Unauthenticated);
    assert_eq!(policy_error(429,b"ignored"),E::Capacity);
    assert_eq!(E::Transport.to_string(),"egress_policy_transport_unresolved");
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn real_reserved_sdk_preserves_e1_receipts_retries_and_revoke() {
    let mut fixture = Store::owned_control_fixture_for_test(15);
    let (bundle,credentials,tokens) = fixture.take_startup();
    let namespace = bundle.owner.closure().sources.iter().find(|s|s.tenant_id.as_deref()==Some("A")).unwrap().durable_namespace_id.to_string();
    let state = GatewayState::for_owned_human_sources(bundle,credentials,tokens,Policy::default(),None,Redactor::new(vec![])).unwrap();
    let retirement = state.human_retirement().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}",listener.local_addr().unwrap());
    let (stop,stopped) = tokio::sync::oneshot::channel::<()>();
    let mut task = tokio::spawn(async move {
        axum::serve(listener,gateway_router(state)).with_graceful_shutdown(async {let _=stopped.await;}).await
    });
    let client = AuditClient::new(&origin,"synthetic-A-runner").unwrap();
    let exercised = std::panic::AssertUnwindSafe(async {
    let diagnostic = client.control_supervision_identity().await.unwrap();
    assert_eq!(diagnostic.schema_version,15);
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    let bytes = serde_json::to_vec(&json!({
        "version":1,"expected_scope":{"tenant_id":"A","control_namespace_uuid":namespace},
        "operation_id":uuid::Uuid::new_v4(),"selector":{"kind":"tenant"},"expected_revision":0,"expected_epoch":0,
        "policy":{"not_before_ms":now.saturating_sub(60_000),"expires_at_ms":now+600_000,
            "destinations":[{"name":"example.com","port":443,"transport":"connect_tcp_opaque"}],
            "max_destinations":1,"max_resolution_ips":1},"tenant_ceiling":null
    })).unwrap();
    let replace = EgressReplacePolicyRequestV1::from_json(&bytes).unwrap();
    let first = client.egress_replace_policy(&replace).await.unwrap();
    let replay = client.egress_replace_policy(&replace).await.unwrap();
    assert_eq!(first.canonical_bytes().unwrap(),replay.canonical_bytes().unwrap());
    let mut wrong = serde_json::to_value(&replace).unwrap();
    wrong["expected_scope"]["tenant_id"]=json!("B");
    let wrong = EgressReplacePolicyRequestV1::from_json(&serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert_eq!(client.egress_replace_policy(&wrong).await.unwrap_err(),E::ScopeConflict);
    let revoke = EgressRevokePolicyRequestV1::new(uuid::Uuid::new_v4(),replace.expected_scope().clone(),EgressPolicySelectorV1::Tenant,1,1).unwrap();
    let revoked = client.egress_revoke_policy(&revoke).await.unwrap();
    let replay = client.egress_revoke_policy(&revoke).await.unwrap();
    assert_eq!(revoked.canonical_bytes().unwrap(),replay.canonical_bytes().unwrap());
    assert_eq!(revoked.state(),EgressPolicyStateV1::RevokeRequested);
    }).catch_unwind().await;
    drop(client);
    let _ = stop.send(());
    let orderly = match tokio::time::timeout(Duration::from_secs(30),&mut task).await {
        Ok(result) => result.is_ok_and(|result|result.is_ok()),
        Err(_) => {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(30),&mut task).await;
            false
        }
    };
    retirement.retire();
    tokio::time::timeout(Duration::from_secs(30),retirement.drain()).await.unwrap().unwrap();
    assert!(orderly,"SDK fixture server did not drain normally");
    if let Err(panic) = exercised { std::panic::resume_unwind(panic); }
}
