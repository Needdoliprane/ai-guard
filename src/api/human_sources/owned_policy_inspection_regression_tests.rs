//! Supplemental inspection regressions; the five baseline parents stay intact.
use super::owned_policy_inspection_tests::{
    Fixture, INSPECT, RUNNER, Server, raw, replacement, selector, snapshot, tenant,
};
use crate::{
    api::gateway_router,
    control::egress_policy::{
        EgressPolicyInspectionRequestV1, EgressPolicyObservedCeilingV1,
        EgressPolicyObservedHeadV1, EgressPolicyObservedSessionResourceV1,
        EgressPolicySelectorV1, EgressPolicyStateV1, EgressRevokePolicyRequestV1,
    },
    runner::AuditClient,
};
use anyhow::{Context, Result, ensure};
use axum::{body::Body, http::{StatusCode, header}};
use serde_json::json;
use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};
use tower::ServiceExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_inspection_reports_stored_revoke_head_without_writing() -> Result<()> {
    let fixture = Fixture::new(15);
    let mut server = Server::new(&fixture.state).await?;
    let outcome: Result<()> = async {
        server.sdk_witness(&fixture, RUNNER, "A", 15).await?;
        let sdk = AuditClient::new(&server.origin, RUNNER)?;
        let request = EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant)?;
        let before = snapshot(&fixture)?;
        let absent = sdk.egress_inspect_policy(&request).await?;
        ensure!(
            absent.scope() == &fixture.scope()
                && absent.head() == &EgressPolicyObservedHeadV1::Absent
                && absent.tenant_ceiling() == &EgressPolicyObservedCeilingV1::NotApplicable
                && absent.session_resource() == EgressPolicyObservedSessionResourceV1::NotApplicable
                && absent.historical()
                && absent.coverage() == "policy_registry_metadata",
            "SDK altered actual empty metadata"
        );
        ensure!(snapshot(&fixture)? == before, "SDK inspection wrote empty metadata");
        sdk.egress_replace_policy(&replacement(&fixture, 0, 0)).await?;
        sdk.egress_revoke_policy(&EgressRevokePolicyRequestV1::new(
            uuid::Uuid::new_v4(), fixture.scope(), EgressPolicySelectorV1::Tenant, 1, 1,
        )?).await?;
        let before = snapshot(&fixture)?;
        let revoked = sdk.egress_inspect_policy(&request).await?;
        ensure!(
            revoked.head() == &EgressPolicyObservedHeadV1::Present {
                revision: 1, epoch: 2, state: EgressPolicyStateV1::RevokeRequested,
            },
            "stored revoke_requested head was omitted or relabeled"
        );
        revoked.validate_for(&request)?;
        ensure!(snapshot(&fixture)? == before, "SDK inspection wrote revoked metadata");
        Ok(())
    }.await;
    let close = server.close().await;
    let drain = fixture.drain().await;
    close?;
    drain?;
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_closes_nested_selectors_and_uses_existing_runner_slots_before_body() -> Result<()> {
    let fixture = Fixture::new(15);
    let mut server = Server::new(&fixture.state).await?;
    let outcome: Result<()> = async {
        server.sdk_witness(&fixture, RUNNER, "A", 15).await?;
        let sdk = AuditClient::new(&server.origin, RUNNER)?;
        sdk.egress_inspect_policy(&EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant)?).await?;
        let before = snapshot(&fixture)?;
        for selected in [
            json!({"kind":"tenant","session_id":"11000000-0000-4000-8000-000000000103"}),
            json!({"kind":"tenant","principal":"synthetic-untrusted"}),
            json!({"kind":"session","session_id":"11000000-0000-4000-8000-000000000103","tenant_id":"B"}),
        ] {
            let (status, _, bytes) = raw(&server.origin, "POST", INSPECT, Some(RUNNER),
                json!({"version":1,"selector":selected})).await?;
            ensure!(status == 400 && serde_json::from_slice::<serde_json::Value>(&bytes)?.get("head").is_none(),
                "nested selector authority was accepted");
        }
        let operation = fixture.state.tenants[tenant(&fixture.state, "A")]
            .control_capacity.runner_semaphore_for_test();
        ensure!(operation.available_permits() == 16, "unexpected actual runner capacity");
        let held = Arc::clone(&operation).try_acquire_many_owned(16)?;
        let polls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&polls);
        let body = futures_util::stream::poll_fn(move |_| {
            count.fetch_add(1, Ordering::AcqRel);
            std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
        });
        let request = axum::http::Request::builder().method("POST").uri(INSPECT)
            .header(header::AUTHORIZATION, format!("Bearer {RUNNER}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, "1024")
            .body(Body::from_stream(body))?;
        let response = tokio::time::timeout(Duration::from_secs(120),
            gateway_router(fixture.state.clone()).oneshot(request)).await
            .context("runner capacity watchdog is not a refusal")??;
        ensure!(response.status() == StatusCode::TOO_MANY_REQUESTS
            && polls.load(Ordering::Acquire) == 0, "full runner capacity polled or admitted body");
        drop(response);
        drop(held);
        ensure!(snapshot(&fixture)? == before, "invalid/capacity refusal changed registry");
        sdk.egress_inspect_policy(&EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Tenant)?).await?;
        let (_, _, bytes) = raw(&server.origin, "POST", INSPECT, Some(RUNNER),
            json!({"version":1,"selector":selector(None)})).await?;
        ensure!(serde_json::from_slice::<serde_json::Value>(&bytes)?["head"] == json!({"presence":"absent"}),
            "capacity release did not recover exact empty metadata");
        Ok(())
    }.await;
    let close = server.close().await;
    let drain = fixture.drain().await;
    close?;
    drain?;
    outcome
}
